import test from "node:test";
import assert from "node:assert/strict";
import dgram from "node:dgram";
import { setTimeout as sleep } from "node:timers/promises";
import { Client, Server, ServerCredentials, type ServerDuplexStream } from "@grpc/grpc-js";
import { parse } from "smol-toml";
import { ShredReceiver, YellowstoneSubscription, defaultRequest, parseClientConfig, SubscribeRequest, SubscribeUpdate } from "../src/index.js";

test("TOML and SDK configuration reject misspelled fields and invalid values", async () => {
  const valid = { shred: { listen: "127.0.0.1:0" }, yellowstone: { endpoint: "http://127.0.0.1:10000" } };
  assert.deepEqual(parseClientConfig(valid), valid);
  assert.throws(() => parseClientConfig(parse('[shred]\nlisten="127.0.0.1:0"\nallowed_source=["192.0.2.1"]\n[yellowstone]\nendpoint="http://127.0.0.1:10000"')), /Unknown config key: shred.allowed_source/);
  for (const [key, value] of [
    ["allowed_sources", null], ["allowed_sources", "127.0.0.1"], ["allowed_sources", [12]],
    ["allowed_sources", ["not-an-ip"]], ["receive_buffer", 0], ["channel_capacity", "4096"],
  ]) {
    assert.throws(() => parseClientConfig({ ...valid, shred: { ...valid.shred, [key as string]: value } }));
  }
  assert.throws(() => parseClientConfig({ ...valid, unknown: true }), /Unknown config key/);
  assert.throws(() => parseClientConfig({ ...valid, yellowstone: { ...valid.yellowstone, x_token: "typo" } }), /Unknown config key/);
  assert.throws(() => parseClientConfig({ ...valid, yellowstone: null }));
  await assert.rejects(ShredReceiver.bind({ ...valid.shred, ...{ allowed_source: ["192.0.2.1"] } }), /Unknown config key/);
  await assert.rejects(YellowstoneSubscription.connect({ ...valid.yellowstone, ...{ x_token: "typo" } }, defaultRequest()), /Unknown config key/);
});

test("dual-stack UDP allowlist matches equivalent IPv4 forms and denies other IPs", {timeout:5000}, async () => {
  for (const allowed of ["127.0.0.1", "::ffff:127.0.0.1", "::ffff:7f00:1", "0:0:0:0:0:FFFF:7F00:1"]) {
    const receiver = await ShredReceiver.bind({listen:"[::]:0", allowed_sources:[allowed]});
    const sender = dgram.createSocket("udp4");
    const denied = dgram.createSocket("udp6");
    try {
      sender.send(Buffer.from("mapped"), receiver.localAddress().port, "127.0.0.1");
      assert.equal((await receiver.next()).value?.data.toString(), "mapped");
      denied.send(Buffer.from("denied"), receiver.localAddress().port, "::1");
      const pending = receiver.next();
      await sleep(50); receiver.close();
      assert.equal((await pending).done, true);
    } finally { receiver.close(); sender.close(); denied.close(); }
  }
});

test("UDP delivers complete packets, bounds queue and closes", {timeout:5000}, async () => {
  const receiver = await ShredReceiver.bind({listen:"127.0.0.1:0", allowed_sources:["127.0.0.1"], channel_capacity:1});
  const sender = dgram.createSocket("udp4");
  try {
    for (const bytes of [Buffer.alloc(0), Buffer.from([0,255,2])]) {
      sender.send(bytes, receiver.localAddress().port, "127.0.0.1");
      const packet = await receiver.next(); assert.deepEqual(packet.value?.data,bytes);
    }
    for (let i=0;i<32;i++) sender.send(Buffer.from([i]),receiver.localAddress().port,"127.0.0.1");
    for (let i=0;i<100&&receiver.droppedPackets===0;i++) await sleep(5);
    assert.ok(receiver.droppedPackets>0);
  } finally { receiver.close();sender.close(); }
});
test("UDP allowlist rejects other sources", {timeout:5000}, async () => {
  const receiver = await ShredReceiver.bind({listen:"127.0.0.1:0",allowed_sources:["192.0.2.1"]});
  const sender=dgram.createSocket("udp4");
  try {
    sender.send(Buffer.from("denied"),receiver.localAddress().port,"127.0.0.1");
    const pending=receiver.next(); await sleep(100);receiver.close();
    assert.equal((await pending).done,true);
  } finally {receiver.close();sender.close();}
});
test("UDP ring wraps in FIFO order, drops newest and drains accepted packets on close", {timeout:5000}, async () => {
  const receiver = await ShredReceiver.bind({listen:"127.0.0.1:0", channel_capacity:3});
  const sender = dgram.createSocket("udp4");
  const expected: number[] = [];
  let sequence = 0;
  try {
    for (let round = 0; round < 8; round++) {
      while (expected.length < 3) {
        const value = sequence++;
        expected.push(value);
        sender.send(Buffer.from([value]), receiver.localAddress().port, "127.0.0.1");
      }
      sender.send(Buffer.from([255]), receiver.localAddress().port, "127.0.0.1");
      // Observing the overflow confirms the entire ordered local burst was received.
      for (let i = 0; i < 100 && receiver.droppedPackets <= round; i++) await sleep(5);
      assert.equal(receiver.droppedPackets, round + 1);
      for (let i = 0; i < 2; i++) {
        assert.equal((await receiver.next()).value?.data[0], expected.shift());
      }
    }
    receiver.close();
    assert.equal((await receiver.next()).value?.data[0], expected.shift());
    assert.equal((await receiver.next()).done, true);
    assert.equal((await receiver.next()).done, true);
  } finally { receiver.close(); sender.close(); }
});
test("Yellowstone preserves HTTP/HTTPS ports for DNS, IPv4 and IPv6 endpoints", async t => {
  const stop = new Error("stop before opening a network connection");
  let target = "";
  t.mock.method(Client.prototype, "waitForReady", function (this: Client, _deadline: Date | number, callback: (error?: Error) => void) {
    target = this.getChannel().getTarget();
    callback(stop);
  });
  for (const host of ["localhost", "127.0.0.1", "[::1]"]) {
    for (const [endpoint, port] of [
      [`http://${host}:80`, "80"], [`http://${host}`, "80"],
      [`https://${host}:443`, "443"], [`https://${host}`, "443"],
      [`http://${host}:10000`, "10000"], [`https://${host}:10000`, "10000"],
      [`http://${host}:443`, "443"], [`https://${host}:80`, "80"],
    ]) {
      await assert.rejects(YellowstoneSubscription.connect({ endpoint }, defaultRequest()), error => error === stop);
      assert.equal(target, `dns:${host}:${port}`, endpoint);
    }
  }
});
test("Yellowstone delivers decoded updates, token, ping, filter changes and EOF", {timeout:10000}, async () => {
  const server = new Server();
  server.addService({subscribe:{
    path:"/geyser.Geyser/Subscribe",requestStream:true,responseStream:true,
    requestSerialize:(r:SubscribeRequest)=>Buffer.from(SubscribeRequest.encode(r).finish()),
    requestDeserialize:(b:Buffer)=>SubscribeRequest.decode(b),
    responseSerialize:(r:SubscribeUpdate)=>Buffer.from(SubscribeUpdate.encode(r).finish()),
    responseDeserialize:(b:Buffer)=>SubscribeUpdate.decode(b),
  }},{subscribe:(call:ServerDuplexStream<SubscribeRequest,SubscribeUpdate>)=>{
    assert.deepEqual(call.metadata.get("x-token"),["test-token"]);
    let step=0;
    call.on("data",(request:SubscribeRequest)=>{
      if(step++===0){assert.ok(request.slots["bot-slots"]);call.write(SubscribeUpdate.fromPartial({ping:{}}));}
      else if(request.ping){call.write(SubscribeUpdate.fromPartial({slot:{slot:"42"}}));}
      else{assert.ok(request.transactions["bot-transactions"]);call.write(SubscribeUpdate.fromPartial({slot:{slot:"43"}}));call.end();}
    });
  }});
  const port=await new Promise<number>((resolve,reject)=>server.bindAsync("127.0.0.1:0",ServerCredentials.createInsecure(),(error,port)=>error?reject(error):resolve(port)));
  process.env.TS_TEST_X_TOKEN="test-token";
  let sub:YellowstoneSubscription|undefined;
  try {
    sub=await YellowstoneSubscription.connect({endpoint:`http://127.0.0.1:${port}`,x_token_env:"TS_TEST_X_TOKEN"},defaultRequest());
    assert.ok((await sub.next()).value?.ping);
    assert.equal((await sub.next()).value?.slot?.slot,"42");
    await sub.updateFilters(defaultRequest(["account"]));
    assert.equal((await sub.next()).value?.slot?.slot,"43");
    assert.equal((await sub.next()).done,true);
  } finally {sub?.close();server.forceShutdown();delete process.env.TS_TEST_X_TOKEN;}
});


test("API key config validates before connecting without leaking values", async () => {
  for (const config of [
    { api_key: "secret\n" }, { api_key: "with space" }, { api_key: "x".repeat(257) },
    { api_key_env: "TS_TEST_MISSING_BRIDGE_KEY" }, { api_key: "secret", api_key_env: "SOME_KEY" },
  ]) {
    await assert.rejects(YellowstoneSubscription.connect({ endpoint: "http://127.0.0.1:1", ...config }, defaultRequest()),
      (error: Error) => /API key|api_key/.test(error.message) && !error.message.includes("secret"));
  }
});

test("API keys reach gRPC metadata; empty keys omit the header", {timeout:10000}, async () => {
  const server = new Server();
  let expected: string[] = [];
  server.addService({subscribe:{
    path:"/geyser.Geyser/Subscribe",requestStream:true,responseStream:true,
    requestSerialize:(r:SubscribeRequest)=>Buffer.from(SubscribeRequest.encode(r).finish()),
    requestDeserialize:(b:Buffer)=>SubscribeRequest.decode(b),
    responseSerialize:(r:SubscribeUpdate)=>Buffer.from(SubscribeUpdate.encode(r).finish()),
    responseDeserialize:(b:Buffer)=>SubscribeUpdate.decode(b),
  }},{subscribe:(call:ServerDuplexStream<SubscribeRequest,SubscribeUpdate>)=>{
    assert.deepEqual(call.metadata.get("x-api-key"),expected);
    assert.deepEqual(call.metadata.get("x-token"),[]);
    call.once("data",()=>{call.write(SubscribeUpdate.fromPartial({slot:{slot:"42"}}));call.end();});
  }});
  const port=await new Promise<number>((resolve,reject)=>server.bindAsync("127.0.0.1:0",ServerCredentials.createInsecure(),(error,port)=>error?reject(error):resolve(port)));
  process.env.TS_TEST_API_KEY="bot-secret";
  process.env.TS_TEST_EMPTY_API_KEY="";
  try {
    for(const [config, values] of [
      [{api_key:""},[]], [{api_key:"bot-secret"},["bot-secret"]],
      [{api_key_env:"TS_TEST_API_KEY"},["bot-secret"]], [{api_key_env:"TS_TEST_EMPTY_API_KEY"},[]],
    ] as const) {
      expected=[...values];
      const sub=await YellowstoneSubscription.connect({endpoint:`http://127.0.0.1:${port}`,...config},defaultRequest());
      try { assert.equal((await sub.next()).value?.slot?.slot,"42"); }
      finally {sub.close();}
    }
  } finally {server.forceShutdown();delete process.env.TS_TEST_API_KEY;delete process.env.TS_TEST_EMPTY_API_KEY;}
});
