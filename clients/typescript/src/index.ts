import dgram from "node:dgram";
import { isIP, type AddressInfo } from "node:net";
import { readFileSync } from "node:fs";
import { Client, credentials, Metadata, type ClientDuplexStream } from "@grpc/grpc-js";
import { SubscribeRequest, SubscribeUpdate } from "@triton-one/yellowstone-grpc";
export { SubscribeRequest, SubscribeUpdate };

export interface ShredConfig {
  listen: string;
  allowed_sources?: string[];
  receive_buffer?: number;
  channel_capacity?: number;
}
export interface ShredPacket { data: Buffer; peer: dgram.RemoteInfo; receivedAt: Date }
function normalizedIP(ip: string): string {
  if (!isIP(ip)) throw new Error(`Invalid IP: ${ip}`);
  if (ip.startsWith("::ffff:") && isIP(ip.slice(7)) === 4) return ip.slice(7);
  return isIP(ip) === 6 ? new URL(`http://[${ip}]`).hostname : ip;
}

/** Single-consumer async iterator. A full UDP queue drops the newest packet. */
export class ShredReceiver implements AsyncIterable<ShredPacket> {
  private readonly queue: ShredPacket[] = [];
  private pending?: { resolve: (value: IteratorResult<ShredPacket>) => void; reject: (error: Error) => void };
  private ended = false;
  private failure?: Error;
  droppedPackets = 0;
  private constructor(private readonly socket: dgram.Socket, private readonly capacity: number, allowed: Set<string>) {
    socket.on("message", (data, peer) => {
      if (allowed.size && !allowed.has(normalizedIP(peer.address))) return;
      const packet = { data, peer, receivedAt: new Date() };
      if (this.pending) { const p = this.pending; this.pending = undefined; p.resolve({ done: false, value: packet }); }
      else if (this.queue.length < capacity) this.queue.push(packet);
      else this.droppedPackets++;
    });
    socket.on("error", error => { this.failure = error; this.close(); });
    socket.on("close", () => this.finish());
  }
  static async bind(config: ShredConfig): Promise<ShredReceiver> {
    const capacity = config.channel_capacity ?? 4096;
    const receiveBuffer = config.receive_buffer ?? 8388608;
    if (!Number.isSafeInteger(capacity) || capacity < 1 || !Number.isSafeInteger(receiveBuffer) || receiveBuffer < 1) throw new Error("Invalid buffer/capacity");
    const address = new URL(`udp://${config.listen}`);
    const host = address.hostname.replace(/^\[|\]$/g, "");
    const port = Number(address.port);
    if (!isIP(host) || !address.port || port < 0 || port > 65535) throw new Error("listen must be IP:port");
    const allowed = new Set((config.allowed_sources ?? []).map(normalizedIP));
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
    const value = this.queue.shift();
    if (value) return { done: false, value };
    if (this.ended) return { done: true, value: undefined };
    if (this.pending) throw new Error("Only one ShredReceiver reader is supported");
    return new Promise((resolve, reject) => { this.pending = { resolve, reject }; });
  }
  [Symbol.asyncIterator](): AsyncIterator<ShredPacket> {
    return { next: () => this.next(), return: async () => { this.close(); return { done: true, value: undefined }; } };
  }
}

export interface YellowstoneConfig {
  endpoint: string;
  x_token_env?: string;
  tls_domain?: string;
  ca_file?: string;
  transaction_accounts?: string[];
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
    const url = new URL(config.endpoint);
    if (!["http:", "https:"].includes(url.protocol) || url.pathname !== "/" || url.search || url.username) throw new Error("endpoint must be http(s)://host:port");
    if (url.protocol === "http:" && (config.tls_domain || config.ca_file)) throw new Error("TLS options require https endpoint");
    const tls = url.protocol === "https:";
    const channel = new Client(url.host, tls ? credentials.createSsl(config.ca_file ? readFileSync(config.ca_file) : undefined) : credentials.createInsecure(), {
      "grpc.max_receive_message_length": 64 * 1024 * 1024,
      ...(config.tls_domain ? { "grpc.ssl_target_name_override": config.tls_domain, "grpc.default_authority": config.tls_domain } : {}),
    });
    try {
      const metadata = new Metadata();
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
