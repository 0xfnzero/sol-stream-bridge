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
	if len(md.Get("x-api-key")) != 0 {
		return status.Error(codes.Unauthenticated, "downstream key leaked upstream")
	}
	request, err := stream.Recv()
	if err != nil {
		return err
	}
	requests := make(chan *pb.SubscribeRequest, 1)
	go func() {
		defer close(requests)
		for {
			r, err := stream.Recv()
			if err != nil {
				return
			}
			select {
			case requests <- r:
			case <-stream.Context().Done():
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
		case r, ok := <-requests:
			if !ok {
				requests = nil
				continue
			}
			if r.Ping == nil {
				request = r
			}
		case <-ticker.C:
			slots := make([]string, 0, len(request.Slots))
			for name := range request.Slots {
				slots = append(slots, name)
			}
			transactions := make([]string, 0, len(request.Transactions))
			for name := range request.Transactions {
				transactions = append(transactions, name)
			}
			if err := stream.Send(&pb.SubscribeUpdate{Filters: slots, UpdateOneof: &pb.SubscribeUpdate_Slot{Slot: &pb.SubscribeUpdateSlot{Slot: 4242}}}); err != nil {
				return err
			}
			if err := stream.Send(&pb.SubscribeUpdate{Filters: transactions, UpdateOneof: &pb.SubscribeUpdate_Transaction{Transaction: &pb.SubscribeUpdateTransaction{Slot: 4242, Transaction: &pb.SubscribeUpdateTransactionInfo{Signature: []byte{1, 2, 3}}}}}); err != nil {
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
