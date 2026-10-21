#!/usr/bin/env node
import { readFileSync } from "node:fs";
import { parse } from "smol-toml";
import { ShredReceiver, YellowstoneSubscription, defaultRequest, parseClientConfig } from "../src/index.js";

const config = parseClientConfig(parse(readFileSync(process.argv[2] ?? "../client.toml", "utf8")));
let shreds: ShredReceiver | undefined;
let grpc: YellowstoneSubscription | undefined;
let stopping = false;
const close = () => { stopping = true; shreds?.close(); grpc?.close(); };
process.once("SIGINT", close); process.once("SIGTERM", close);
try {
  shreds = await ShredReceiver.bind(config.shred);
  const packets = shreds;
  // Start consuming UDP immediately, even while gRPC is connecting.
  const receiveShreds = async () => {
    for await (const packet of packets) {
      console.log(`shred: peer=${packet.peer.address}:${packet.peer.port} bytes=${packet.data.length} drops=${packets.droppedPackets}`);
      // Feed packet.data to your Bot's shred parser here.
    }
  };
  const receiveGrpc = async () => {
    grpc = await YellowstoneSubscription.connect(config.yellowstone, defaultRequest(config.yellowstone.transaction_accounts));
    if (stopping) { grpc.close(); return; }
    for await (const update of grpc) {
      if (update.slot) console.log(`slot: ${update.slot.slot}`);
      if (update.transaction) console.log(`transaction: slot=${update.transaction.slot} signature=${Buffer.from(update.transaction.transaction?.signature ?? []).toString("hex")}`);
      // Invoke Bot strategy with update.account / update.transaction / update.slot.
    }
    if (!stopping) throw new Error("Yellowstone stream ended; reconnect and resubscribe");
  };
  await Promise.all([receiveShreds(), receiveGrpc()]);
} catch (error) {
  if (!stopping) { console.error(error); process.exitCode = 1; }
} finally { close(); }
