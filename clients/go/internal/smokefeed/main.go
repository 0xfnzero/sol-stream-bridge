// Local integration fixture; never needed by production bots.
package main

import (
	"flag"
	pb "github.com/rpcpool/yellowstone-grpc/examples/golang/proto"
	"google.golang.org/grpc"
	"google.golang.org/grpc/codes"
	"google.golang.org/grpc/metadata"
	"google.golang.org/grpc/status"
	"log"
	"net"
	"time"
)

type feed struct{ pb.UnimplementedGeyserServer }

func (feed) Subscribe(stream pb.Geyser_SubscribeServer) error {
	md, _ := metadata.FromIncomingContext(stream.Context())
	if len(md.Get("x-token")) != 1 || md.Get("x-token")[0] != "smoke-token" {
		return status.Error(codes.Unauthenticated, "token missing")
	}
	if _, err := stream.Recv(); err != nil {
		return err
	}
	// Drain requests so client cancellation and request-side flow control remain normal.
	go func() {
		for {
			if _, err := stream.Recv(); err != nil {
				return
			}
		}
	}()
	ticker := time.NewTicker(100 * time.Millisecond)
	defer ticker.Stop()
	for {
		select {
		case <-stream.Context().Done():
			return stream.Context().Err()
		case <-ticker.C:
			if err := stream.Send(&pb.SubscribeUpdate{UpdateOneof: &pb.SubscribeUpdate_Slot{Slot: &pb.SubscribeUpdateSlot{Slot: 4242}}}); err != nil {
				return err
			}
			if err := stream.Send(&pb.SubscribeUpdate{UpdateOneof: &pb.SubscribeUpdate_Transaction{Transaction: &pb.SubscribeUpdateTransaction{Slot: 4242, Transaction: &pb.SubscribeUpdateTransactionInfo{Signature: []byte{1, 2, 3}}}}}); err != nil {
				return err
			}
		}
	}
}
func main() {
	address := flag.String("listen", "127.0.0.1:18000", "listen")
	flag.Parse()
	l, err := net.Listen("tcp", *address)
	if err != nil {
		log.Fatal(err)
	}
	server := grpc.NewServer()
	pb.RegisterGeyserServer(server, feed{})
	log.Fatal(server.Serve(l))
}
