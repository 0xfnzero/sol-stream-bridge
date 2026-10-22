import { resolveApiKey } from "./config.js";
import dgram from "node:dgram";
import { BlockList, isIP, type AddressInfo } from "node:net";
import { readFileSync } from "node:fs";
import { Client, credentials, Metadata, type ClientDuplexStream } from "@grpc/grpc-js";
import { SubscribeRequest, SubscribeUpdate } from "@triton-one/yellowstone-grpc";
import { validateShredConfig, validateYellowstoneConfig, type ShredConfig, type YellowstoneConfig } from "./config.js";
export { parseClientConfig, type ClientConfig, type ShredConfig, type YellowstoneConfig } from "./config.js";
export { SubscribeRequest, SubscribeUpdate };

export interface ShredPacket { data: Buffer; peer: dgram.RemoteInfo; receivedAt: Date }

/** Single-consumer async iterator. A full UDP queue drops the newest packet. */
export class ShredReceiver implements AsyncIterable<ShredPacket> {
  // Ring indices avoid shifting every retained packet on each read.
  private readonly queue: (ShredPacket | undefined)[] = [];
  private head = 0;
  private queued = 0;
  private pending?: { resolve: (value: IteratorResult<ShredPacket>) => void; reject: (error: Error) => void };
  private ended = false;
  private failure?: Error;
  droppedPackets = 0;
  private constructor(private readonly socket: dgram.Socket, private readonly capacity: number, allowed?: BlockList) {
    socket.on("message", (data, peer) => {
      if (allowed && !allowed.check(peer.address, peer.family === "IPv6" ? "ipv6" : "ipv4")) return;
      if (this.ended) return;
      if (!this.pending && this.queued === capacity) { this.droppedPackets++; return; }
      const packet = { data, peer, receivedAt: new Date() };
      if (this.pending) { const p = this.pending; this.pending = undefined; p.resolve({ done: false, value: packet }); }
      else {
        this.queue[(this.head + this.queued) % capacity] = packet;
        this.queued++;
      }
    });
    socket.on("error", error => { this.failure = error; this.close(); });
    socket.on("close", () => this.finish());
  }
  static async bind(config: ShredConfig): Promise<ShredReceiver> {
    validateShredConfig(config);
    const capacity = config.channel_capacity ?? 4096;
    const receiveBuffer = config.receive_buffer ?? 8388608;
    const address = new URL(`udp://${config.listen}`);
    const host = address.hostname.replace(/^\[|\]$/g, "");
    const port = Number(address.port);
    if (!isIP(host) || !address.port || port < 0 || port > 65535) throw new Error("listen must be IP:port");
    // Node compares address bytes, including every IPv4-mapped IPv6 spelling.
    const allowed = config.allowed_sources?.length ? new BlockList() : undefined;
    for (const ip of config.allowed_sources ?? []) allowed!.addAddress(ip, isIP(ip) === 6 ? "ipv6" : "ipv4");
    const socket = dgram.createSocket(isIP(host) === 6 ? "udp6" : "udp4");
    const receiver = new ShredReceiver(socket, capacity, allowed);
    try {
      await new Promise<void>((resolve, reject) => {
        const onError = (error: Error) => reject(error);
        socket.once("error", onError);
        socket.bind(port, host, () => { socket.off("error", onError); resolve(); });
      });
      socket.setRecvBufferSize(receiveBuffer);
      return receiver;
    } catch (error) { receiver.close(); throw error; }
  }
  localAddress(): AddressInfo { return this.socket.address(); }
  private finish(): void {
    this.ended = true;
    if (this.failure) { this.queue.length = 0; this.queued = 0; this.head = 0; }
    if (this.pending) {
      const p = this.pending; this.pending = undefined;
      if (this.failure) p.reject(this.failure); else p.resolve({ done: true, value: undefined });
    }
  }
  close(): void {
    if (this.ended) return;
    this.finish();
    try { this.socket.close(); } catch { /* A failed bind may already be closed. */ }
  }
  async next(): Promise<IteratorResult<ShredPacket>> {
    if (this.failure) throw this.failure;
    if (this.queued) {
      const value = this.queue[this.head]!;
      this.queue[this.head] = undefined;
      this.head = (this.head + 1) % this.capacity;
      this.queued--;
      return { done: false, value };
    }
    if (this.ended) return { done: true, value: undefined };
    if (this.pending) throw new Error("Only one ShredReceiver reader is supported");
    return new Promise((resolve, reject) => { this.pending = { resolve, reject }; });
  }
  [Symbol.asyncIterator](): AsyncIterator<ShredPacket> {
    return { next: () => this.next(), return: async () => { this.close(); return { done: true, value: undefined }; } };
  }
}

/** Complete replacement request; callers may add account/block/entry filters. */
export function defaultRequest(accounts: string[] = []): SubscribeRequest {
  return SubscribeRequest.fromPartial({
    slots: { "bot-slots": {} },
    transactions: accounts.length ? { "bot-transactions": { vote: false, failed: false, accountInclude: accounts } } : {},
  });
}
export class YellowstoneSubscription implements AsyncIterable<SubscribeUpdate> {
  private readonly iterator: AsyncIterator<SubscribeUpdate>;
  private constructor(private readonly client: Client, private readonly stream: ClientDuplexStream<SubscribeRequest, SubscribeUpdate>) {
    // Keep errors observable even when the Bot is between reads; the iterator also receives them.
    stream.on("error", () => {});
    this.iterator = stream[Symbol.asyncIterator]();
  }
  static async connect(config: YellowstoneConfig, request: SubscribeRequest): Promise<YellowstoneSubscription> {
    validateYellowstoneConfig(config);
    const apiKey = resolveApiKey(config);
    const url = new URL(config.endpoint);
    if (!["http:", "https:"].includes(url.protocol) || url.pathname !== "/" || url.search || url.username) throw new Error("endpoint must be http(s)://host:port");
    if (url.protocol === "http:" && (config.tls_domain || config.ca_file)) throw new Error("TLS options require https endpoint");
    const tls = url.protocol === "https:";
    // URL removes default ports; grpc-js defaults to 443 even for plaintext.
    const target = `${url.hostname}:${url.port || (tls ? "443" : "80")}`;
    const channel = new Client(target, tls ? credentials.createSsl(config.ca_file ? readFileSync(config.ca_file) : undefined) : credentials.createInsecure(), {
      "grpc.max_receive_message_length": 64 * 1024 * 1024,
      ...(config.tls_domain ? { "grpc.ssl_target_name_override": config.tls_domain, "grpc.default_authority": config.tls_domain } : {}),
    });
    try {
      const metadata = new Metadata();
      if (apiKey) metadata.set("x-api-key", apiKey);
      if (config.x_token_env) {
        const token = process.env[config.x_token_env];
        if (token === undefined) throw new Error(`Missing env ${config.x_token_env}`);
        metadata.set("x-token", token);
      }
      await new Promise<void>((resolve, reject) => channel.waitForReady(Date.now() + 10000, error => error ? reject(error) : resolve()));
      const stream = channel.makeBidiStreamRequest<SubscribeRequest, SubscribeUpdate>(
        "/geyser.Geyser/Subscribe", value => Buffer.from(SubscribeRequest.encode(value).finish()),
        bytes => SubscribeUpdate.decode(bytes), metadata,
      );
      const subscription = new YellowstoneSubscription(channel, stream);
      try { await subscription.updateFilters(request); return subscription; }
      catch (error) { subscription.close(); throw error; }
    } catch (error) { channel.close(); throw error; }
  }
  async updateFilters(request: SubscribeRequest): Promise<void> {
    await new Promise<void>((resolve, reject) => this.stream.write(request, (error?: Error | null) => error ? reject(error) : resolve()));
  }
  async next(): Promise<IteratorResult<SubscribeUpdate>> {
    const result = await this.iterator.next();
    if (!result.done && result.value.ping) await this.updateFilters(SubscribeRequest.fromPartial({ ping: { id: 1 } }));
    return result;
  }
  close(): void { this.stream.cancel(); this.client.close(); }
  [Symbol.asyncIterator](): AsyncIterator<SubscribeUpdate> {
    return { next: () => this.next(), return: async () => { this.close(); return { done: true, value: undefined }; } };
  }
}
