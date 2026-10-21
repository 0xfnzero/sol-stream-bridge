#!/usr/bin/env python3
"""Build and run all three examples through local bridge processes and a mock Geyser.
Requires Rust, Go, Node/npm and installed TypeScript dependencies (npm ci).
Uses loopback only; writes all generated config/logs/binaries to a temporary folder.
"""
import os
from pathlib import Path
import signal
import socket
import subprocess
import tempfile
import time

ROOT = Path(__file__).resolve().parents[1]

def free_port(kind=socket.SOCK_STREAM):
    with socket.socket(socket.AF_INET, kind) as sock:
        sock.bind(('127.0.0.1', 0))
        return sock.getsockname()[1]

def ready(port, process):
    for _ in range(100):
        if process.poll() is not None:
            raise RuntimeError('service exited before ready')
        try:
            with socket.create_connection(('127.0.0.1', port), timeout=.1):
                return
        except OSError:
            time.sleep(.05)
    raise RuntimeError(f'port {port} not ready')

def run():
    subprocess.run(['cargo','build','--locked','--features','yellowstone','--bins','--examples'],cwd=ROOT,check=True)
    subprocess.run(['npm','run','build'],cwd=ROOT/'clients/typescript',check=True)
    with tempfile.TemporaryDirectory(prefix='bridge-smoke-') as directory:
        tmp=Path(directory)
        subprocess.run(['go','build','-o',str(tmp/'feed'),'./internal/smokefeed'],cwd=ROOT/'clients/go',check=True)
        subprocess.run(['go','build','-o',str(tmp/'go-receive'),'./examples/receive'],cwd=ROOT/'clients/go',check=True)
        env=dict(os.environ,BRIDGE_TOKEN='local-smoke-token-at-least-32-bytes',YELLOWSTONE_X_TOKEN='smoke-token')
        processes=[]
        handles=[]
        def start(name,command):
            log=tmp/(name+'.log');handle=log.open('w');handles.append(handle)
            process=subprocess.Popen(command,cwd=ROOT,env=env,stdout=handle,stderr=subprocess.STDOUT)
            processes.append(process)
            return process,log
        try:
            upstream,tunnel,public=free_port(),free_port(),free_port()
            udp_in,udp_out=free_port(socket.SOCK_DGRAM),free_port(socket.SOCK_DGRAM)
            feed,_=start('feed',[str(tmp/'feed'),'-listen',f'127.0.0.1:{upstream}']);ready(upstream,feed)
            receiver=tmp/'receiver.toml';receiver.write_text(f'''role="receiver"
[grpc]
tunnel_address="127.0.0.1:{tunnel}"
listen="127.0.0.1:{public}"
allowed_sources=["127.0.0.1"]
token_env="BRIDGE_TOKEN"
insecure=true
''')
            source=tmp/'source.toml';source.write_text(f'''role="source"
[[shred]]
name="smoke"
listen="127.0.0.1:{udp_in}"
targets=["127.0.0.1:{udp_out}"]
allowed_sources=["127.0.0.1"]
[grpc]
tunnel_address="127.0.0.1:{tunnel}"
upstream="127.0.0.1:{upstream}"
token_env="BRIDGE_TOKEN"
insecure=true
reconnect_ms=20
''')
            binary=str(ROOT/'target/debug/stream-bridge-server')
            recv,_=start('receiver',[binary,'--config',str(receiver)]);ready(tunnel,recv)
            start('source',[binary,'--config',str(source)])
            config=tmp/'client.toml';config.write_text(f'''[shred]
listen="127.0.0.1:{udp_out}"
allowed_sources=["127.0.0.1"]
channel_capacity=32
receive_buffer=65536
[yellowstone]
endpoint="http://127.0.0.1:{public}"
x_token_env="YELLOWSTONE_X_TOKEN"
transaction_accounts=["11111111111111111111111111111111"]
''')
            commands={
                'Rust':[str(ROOT/'target/debug/examples/receive'),str(config)],
                'Go':[str(tmp/'go-receive'),str(config)],
                'TypeScript':['node',str(ROOT/'clients/typescript/dist/examples/receive.js'),str(config)],
            }
            with socket.socket(socket.AF_INET,socket.SOCK_DGRAM) as udp:
                for name,command in commands.items():
                    client,log=start(name,command)
                    for _ in range(150):
                        udp.sendto(b'raw-shred-smoke',('127.0.0.1',udp_in))
                        time.sleep(.05)
                        output=log.read_text()
                        if all(text in output for text in ['shred:','slot: 4242','transaction: slot=4242']):
                            break
                        if client.poll() is not None:
                            raise RuntimeError(f'{name} exited:\n{output}')
                    else:
                        raise RuntimeError(f'{name} missing updates:\n{log.read_text()}')
                    client.send_signal(signal.SIGINT)
                    client.wait(timeout=15)
                    if client.returncode != 0:
                        raise RuntimeError(f'{name} shutdown failed:\n{log.read_text()}')
                    print(f'{name}: received UDP shred, Yellowstone slot and transaction through bridge; clean shutdown',flush=True)
        except Exception:
            for log in tmp.glob('*.log'):
                print(f'--- {log.name} ---\n{log.read_text()[-4000:]}')
            raise
        finally:
            for process in reversed(processes):
                if process.poll() is None:
                    process.terminate()
                    try: process.wait(timeout=5)
                    except subprocess.TimeoutExpired: process.kill();process.wait()
            for handle in handles: handle.close()

if __name__=='__main__': run()
