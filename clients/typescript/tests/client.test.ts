import test from "node:test";
import assert from "node:assert/strict";
import dgram from "node:dgram";
import { setTimeout as sleep } from "node:timers/promises";
import { Server, ServerCredentials, type ServerDuplexStream } from "@grpc/grpc-js";
import { ShredReceiver, YellowstoneSubscription, defaultRequest, SubscribeRequest, SubscribeUpdate } from "../src/index.js";

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
