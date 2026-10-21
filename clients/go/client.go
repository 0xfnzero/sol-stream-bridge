// Package bridgeclient exposes raw Shred packets and typed Yellowstone updates to bots.
package bridgeclient

import (
	"context"
	"crypto/tls"
	"crypto/x509"
	"errors"
	"fmt"
	"net"
	"net/url"
	"os"
	"sync"
	"sync/atomic"
	"time"

	pb "github.com/rpcpool/yellowstone-grpc/examples/golang/proto"
	"google.golang.org/grpc"
	"google.golang.org/grpc/credentials"
	"google.golang.org/grpc/credentials/insecure"
	"google.golang.org/grpc/metadata"
)

type ShredConfig struct {
	Listen          string   `toml:"listen"`
	AllowedSources  []string `toml:"allowed_sources"`
	ReceiveBuffer   int      `toml:"receive_buffer"`
	ChannelCapacity int      `toml:"channel_capacity"`
}
type ShredPacket struct {
	Data       []byte
	Peer       *net.UDPAddr
	ReceivedAt time.Time
}
type ShredReceiver struct {
	Packets <-chan ShredPacket
	Errors  <-chan error
	socket  *net.UDPConn
	dropped atomic.Uint64
	stop    func() bool
}

func ListenShreds(ctx context.Context, cfg ShredConfig) (*ShredReceiver, error) {
	if cfg.ChannelCapacity == 0 {
		cfg.ChannelCapacity = 4096
	}
	if cfg.ReceiveBuffer == 0 {
		cfg.ReceiveBuffer = 8 * 1024 * 1024
	}
	if cfg.ChannelCapacity < 1 || cfg.ReceiveBuffer < 1 {
		return nil, errors.New("buffer and capacity must be positive")
	}
	allowed := make([]net.IP, 0, len(cfg.AllowedSources))
	for _, s := range cfg.AllowedSources {
		ip := net.ParseIP(s)
		if ip == nil {
			return nil, fmt.Errorf("invalid allowed IP %q", s)
		}
		allowed = append(allowed, ip)
	}
	addr, err := net.ResolveUDPAddr("udp", cfg.Listen)
	if err != nil {
		return nil, err
	}
	conn, err := net.ListenUDP("udp", addr)
	if err != nil {
		return nil, err
	}
	if err = conn.SetReadBuffer(cfg.ReceiveBuffer); err != nil {
		conn.Close()
		return nil, err
	}
	packets := make(chan ShredPacket, cfg.ChannelCapacity)
	errs := make(chan error, 1)
	r := &ShredReceiver{Packets: packets, Errors: errs, socket: conn}
	r.stop = context.AfterFunc(ctx, func() { conn.Close() })
	go func() {
		defer close(packets)
		defer close(errs)
		defer conn.Close()
		defer r.stop()
		buf := make([]byte, 65535)
		for {
			n, peer, err := conn.ReadFromUDP(buf)
			if err != nil {
				if !errors.Is(err, net.ErrClosed) {
					errs <- err
				}
				return
			}
			accept := len(allowed) == 0
			for _, ip := range allowed {
				if ip.Equal(peer.IP) {
					accept = true
					break
				}
			}
			if !accept {
				continue
			}
			packet := ShredPacket{Data: append([]byte{}, buf[:n]...), Peer: peer, ReceivedAt: time.Now()}
			select {
			case packets <- packet:
			default:
				r.dropped.Add(1)
			}
		}
	}()
	return r, nil
}
func (r *ShredReceiver) Close() error           { r.stop(); return r.socket.Close() }
func (r *ShredReceiver) LocalAddr() net.Addr    { return r.socket.LocalAddr() }
func (r *ShredReceiver) DroppedPackets() uint64 { return r.dropped.Load() }

type YellowstoneConfig struct {
	Endpoint            string   `toml:"endpoint"`
	XTokenEnv           string   `toml:"x_token_env"`
	TLSDomain           string   `toml:"tls_domain"`
	CAFile              string   `toml:"ca_file"`
	TransactionAccounts []string `toml:"transaction_accounts"`
}
type Subscription struct {
	stream pb.Geyser_SubscribeClient
	conn   *grpc.ClientConn
	cancel context.CancelFunc
	sendMu sync.Mutex
}

// Subscribe connects to the receiver's gRPC listen port, never its tunnel port.
// The caller owns ctx; cancellation closes the stream. Recv must have one reader.
func Subscribe(ctx context.Context, cfg YellowstoneConfig, request *pb.SubscribeRequest) (*Subscription, error) {
	u, err := url.Parse(cfg.Endpoint)
	if err != nil {
		return nil, err
	}
	if u.Host == "" || (u.Scheme != "http" && u.Scheme != "https") || (u.Path != "" && u.Path != "/") {
		return nil, errors.New("endpoint must be http(s)://host:port")
	}
	var transport credentials.TransportCredentials
	if u.Scheme == "https" {
		tc := &tls.Config{MinVersion: tls.VersionTLS12, ServerName: cfg.TLSDomain}
		if cfg.CAFile != "" {
			pem, e := os.ReadFile(cfg.CAFile)
			if e != nil {
				return nil, e
			}
			roots, e := x509.SystemCertPool()
			if e != nil {
				roots = x509.NewCertPool()
			}
			if !roots.AppendCertsFromPEM(pem) {
				return nil, errors.New("invalid CA file")
			}
			tc.RootCAs = roots
		}
		transport = credentials.NewTLS(tc)
	} else {
		if cfg.TLSDomain != "" || cfg.CAFile != "" {
			return nil, errors.New("TLS options require https endpoint")
		}
		transport = insecure.NewCredentials()
	}
	conn, err := grpc.NewClient(u.Host, grpc.WithTransportCredentials(transport), grpc.WithDefaultCallOptions(grpc.MaxCallRecvMsgSize(64*1024*1024)))
	if err != nil {
		return nil, err
	}
	ctx, cancel := context.WithCancel(ctx)
	if cfg.XTokenEnv != "" {
		token, ok := os.LookupEnv(cfg.XTokenEnv)
		if !ok {
			cancel()
			conn.Close()
			return nil, fmt.Errorf("missing env %s", cfg.XTokenEnv)
		}
		ctx = metadata.AppendToOutgoingContext(ctx, "x-token", token)
	}
	stream, err := pb.NewGeyserClient(conn).Subscribe(ctx)
	if err != nil {
		cancel()
		conn.Close()
		return nil, err
	}
	s := &Subscription{stream: stream, conn: conn, cancel: cancel}
	if err = s.UpdateFilters(request); err != nil {
		s.Close()
		return nil, err
	}
	return s, nil
}

// Recv returns decoded protobuf messages. EOF/errors are exposed; no silent replay.
func (s *Subscription) Recv() (*pb.SubscribeUpdate, error) {
	update, err := s.stream.Recv()
	if err != nil {
		return nil, err
	}
	if update.GetPing() != nil {
		if err = s.UpdateFilters(&pb.SubscribeRequest{Ping: &pb.SubscribeRequestPing{Id: 1}}); err != nil {
			return nil, err
		}
	}
	return update, nil
}

// UpdateFilters sends a complete replacement filter request. Sends are serialized.
func (s *Subscription) UpdateFilters(request *pb.SubscribeRequest) error {
	if request == nil {
		return errors.New("subscription request must not be nil")
	}
	s.sendMu.Lock()
	defer s.sendMu.Unlock()
	return s.stream.Send(request)
}
func (s *Subscription) Close() error { s.cancel(); return s.conn.Close() }

// DefaultRequest subscribes to slots and, if supplied, transactions matching accounts.
func DefaultRequest(accounts []string) *pb.SubscribeRequest {
	r := &pb.SubscribeRequest{Slots: map[string]*pb.SubscribeRequestFilterSlots{"bot-slots": {}}}
	if len(accounts) > 0 {
		no := false
		r.Transactions = map[string]*pb.SubscribeRequestFilterTransactions{"bot-transactions": {Vote: &no, Failed: &no, AccountInclude: accounts}}
	}
	return r
}
