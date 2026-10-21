package main

import (
	"context"
	"fmt"
	"log"
	"os"
	"os/signal"
	"syscall"

	bridge "github.com/0xfnzero/stream-bridge/clients/go"
	"github.com/BurntSushi/toml"
)

func main() {
	if err := run(); err != nil {
		log.Fatal(err)
	}
}
func run() error {
	path := "../client.toml"
	if len(os.Args) > 1 {
		path = os.Args[1]
	}
	var cfg struct {
		Shred       bridge.ShredConfig       `toml:"shred"`
		Yellowstone bridge.YellowstoneConfig `toml:"yellowstone"`
	}
	md, err := toml.DecodeFile(path, &cfg)
	if err != nil {
		return err
	}
	if len(md.Undecoded()) > 0 {
		return fmt.Errorf("unknown keys: %v", md.Undecoded())
	}
	ctx, cancel := signal.NotifyContext(context.Background(), os.Interrupt, syscall.SIGTERM)
	defer cancel()
	shreds, err := bridge.ListenShreds(ctx, cfg.Shred)
	if err != nil {
		return err
	}
	defer shreds.Close()
	errors := make(chan error, 1)
	go func() {
		sub, err := bridge.Subscribe(ctx, cfg.Yellowstone, bridge.DefaultRequest(cfg.Yellowstone.TransactionAccounts))
		if err != nil {
			errors <- err
			return
		}
		defer sub.Close()
		for {
			update, err := sub.Recv()
			if err != nil {
				errors <- err
				return
			}
			if slot := update.GetSlot(); slot != nil {
				fmt.Printf("slot: %d\n", slot.Slot)
			}
			if tx := update.GetTransaction(); tx != nil {
				fmt.Printf("transaction: slot=%d signature=%x\n", tx.Slot, tx.GetTransaction().GetSignature())
			}
		}
	}()
	for {
		select {
		case <-ctx.Done():
			return nil
		case err := <-errors:
			if ctx.Err() != nil {
				return nil
			}
			return err
		case packet, ok := <-shreds.Packets:
			if !ok {
				if err := <-shreds.Errors; err != nil {
					return err
				}
				return nil
			}
			fmt.Printf("shred: peer=%s bytes=%d drops=%d\n", packet.Peer, len(packet.Data), shreds.DroppedPackets())
		case err, ok := <-shreds.Errors:
			if !ok {
				return nil
			}
			return err
		}
	}
}
