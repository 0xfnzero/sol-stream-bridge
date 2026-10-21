package bridgeclient

import (
	"context"
	"io"
	"net"
	"testing"
	"time"

	pb "github.com/rpcpool/yellowstone-grpc/examples/golang/proto"
	"google.golang.org/grpc"
	"google.golang.org/grpc/codes"
	"google.golang.org/grpc/metadata"
	"google.golang.org/grpc/status"
)

type mockGeyser struct{ pb.UnimplementedGeyserServer }

func (mockGeyser) Subscribe(s pb.Geyser_SubscribeServer) error {
	md, _ := metadata.FromIncomingContext(s.Context())
	if len(md.Get("x-token")) != 1 || md.Get("x-token")[0] != "test-token" {
		return status.Error(codes.Unauthenticated, "token missing")
	}
	first, err := s.Recv()
	if err != nil {
		return err
	}
	if first.Slots["bot-slots"] == nil {
		return status.Error(codes.InvalidArgument, "slots missing")
	}
	if err = s.Send(&pb.SubscribeUpdate{UpdateOneof: &pb.SubscribeUpdate_Ping{Ping: &pb.SubscribeUpdatePing{}}}); err != nil {
		return err
	}
	ping, err := s.Recv()
	if err != nil {
		return err
	}
	if ping.Ping == nil {
		return status.Error(codes.InvalidArgument, "ping reply missing")
	}
	if err = s.Send(&pb.SubscribeUpdate{UpdateOneof: &pb.SubscribeUpdate_Slot{Slot: &pb.SubscribeUpdateSlot{Slot: 42}}}); err != nil {
		return err
	}
	next, err := s.Recv()
	if err != nil {
		return err
	}
	if next.Transactions["bot-transactions"] == nil {
		return status.Error(codes.InvalidArgument, "updated filters missing")
	}
	return s.Send(&pb.SubscribeUpdate{UpdateOneof: &pb.SubscribeUpdate_Slot{Slot: &pb.SubscribeUpdateSlot{Slot: 43}}})
}
func TestSubscriptionDataPingFiltersAndEOF(t *testing.T) {
	t.Setenv("GO_TEST_X_TOKEN", "test-token")
	l, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		t.Fatal(err)
	}
	server := grpc.NewServer()
	pb.RegisterGeyserServer(server, mockGeyser{})
	go server.Serve(l)
	defer server.Stop()
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()
	sub, err := Subscribe(ctx, YellowstoneConfig{Endpoint: "http://" + l.Addr().String(), XTokenEnv: "GO_TEST_X_TOKEN"}, DefaultRequest(nil))
	if err != nil {
		t.Fatal(err)
	}
	defer sub.Close()
	ping, err := sub.Recv()
	if err != nil || ping.GetPing() == nil {
		t.Fatalf("ping: %v %v", ping, err)
	}
	update, err := sub.Recv()
	if err != nil || update.GetSlot().Slot != 42 {
		t.Fatalf("slot: %v %v", update, err)
	}
	if err = sub.UpdateFilters(DefaultRequest([]string{"account"})); err != nil {
		t.Fatal(err)
	}
	update, err = sub.Recv()
	if err != nil || update.GetSlot().Slot != 43 {
		t.Fatalf("new slot: %v %v", update, err)
	}
	if _, err = sub.Recv(); err != io.EOF {
		t.Fatalf("wanted EOF, got %v", err)
	}
}
func TestShredChannelBoundariesOverflowAndClose(t *testing.T) {
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()
	r, err := ListenShreds(ctx, ShredConfig{Listen: "127.0.0.1:0", AllowedSources: []string{"127.0.0.1"}, ChannelCapacity: 1})
	if err != nil {
		t.Fatal(err)
	}
	defer r.Close()
	conn, err := net.Dial("udp", r.LocalAddr().String())
	if err != nil {
		t.Fatal(err)
	}
	defer conn.Close()
	for _, bytes := range [][]byte{{}, {0, 255, 2}} {
		conn.Write(bytes)
		select {
		case p := <-r.Packets:
			if string(p.Data) != string(bytes) {
				t.Fatalf("payload %v", p.Data)
			}
		case <-time.After(time.Second):
			t.Fatal("no packet")
		}
	}
	for i := 0; i < 32; i++ {
		conn.Write([]byte{byte(i)})
	}
	deadline := time.Now().Add(time.Second)
	for r.DroppedPackets() == 0 && time.Now().Before(deadline) {
		time.Sleep(time.Millisecond)
	}
	if r.DroppedPackets() == 0 {
		t.Fatal("overflow not counted")
	}
	cancel()
	for range r.Packets {
	}
}
func TestShredAllowlist(t *testing.T) {
	r, err := ListenShreds(context.Background(), ShredConfig{Listen: "127.0.0.1:0", AllowedSources: []string{"192.0.2.1"}})
	if err != nil {
		t.Fatal(err)
	}
	defer r.Close()
	conn, err := net.Dial("udp", r.LocalAddr().String())
	if err != nil {
		t.Fatal(err)
	}
	defer conn.Close()
	conn.Write([]byte("denied"))
	select {
	case <-r.Packets:
		t.Fatal("unexpected packet")
	case <-time.After(100 * time.Millisecond):
	}
}
