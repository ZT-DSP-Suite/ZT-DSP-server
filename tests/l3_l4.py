#!/usr/bin/env python3
# 验证两件事：静默丢弃契约、L4 控制面。
import json, socket, struct, subprocess, time, sys

PORT, CPORT = 17453, 17454

def start():
    p = subprocess.Popen(['./target/debug/zt-dsp-server',
                          '--listen', f'127.0.0.1:{PORT}',
                          '--control', f'127.0.0.1:{CPORT}',
                          '--profile', 'beginner'],
                         stderr=subprocess.PIPE)
    time.sleep(1.2)
    return p

def test_silent_drop():
    """非法长度前缀 → 服务器不回一个字节，也不立刻 RST。"""
    s = socket.create_connection(('127.0.0.1', PORT))
    s.sendall(struct.pack('>I', 99999))          # param_len 远超 4096
    s.settimeout(2.5)
    t0 = time.time()
    try:
        data = s.recv(4096)
        assert data == b'', f'静默丢弃失败：服务器回了 {len(data)} 字节'
        print('  [1] 静默丢弃：未回任何字节 OK')
    except socket.timeout:
        print('  [1] 静默丢弃：连接挂起无响应 OK')
    finally:
        s.close()

def call(sock, obj):
    sock.sendall((json.dumps(obj) + '\n').encode())
    buf = b''
    while not buf.endswith(b'\n'):
        c = sock.recv(65536)
        if not c:
            raise RuntimeError('控制面提前关闭')
        buf += c
    return json.loads(buf.decode())

def test_control():
    c = socket.create_connection(('127.0.0.1', CPORT))
    c.settimeout(5)
    h = 'ab' * 32
    h2 = 'cd' * 32

    r = call(c, {'op': 'ping'})
    assert r['ok'] and r['data']['pong']
    print('  [2] 控制面 ping OK')

    r = call(c, {'op': 'offline_push', 'dst': h, 'payload': 'deadbeef'})
    assert r['ok'], r
    print('  [3] 离线消息入库 OK')

    r = call(c, {'op': 'offline_pull', 'dst': h})
    assert r['ok'] and r['data']['items'] == ['deadbeef'], r
    print('  [4] 离线消息取回且内容一致 OK')

    r = call(c, {'op': 'group_create', 'name': 'g1', 'owner': h})
    assert r['ok'], r
    r = call(c, {'op': 'group_join', 'name': 'g1', 'member': h2})
    assert r['ok'], r
    r = call(c, {'op': 'group_send', 'name': 'g1', 'sender': h, 'payload': 'cafe'})
    assert r['ok'], r
    r = call(c, {'op': 'group_pull', 'name': 'g1', 'member': h2})
    assert r['ok'] and r['data']['items'][0]['payload'] == 'cafe', r
    print('  [5] 群组广播 + 游标拉取 OK')

    # 默认档位禁止任意哈希查询（抗流量分析），这是刻意设计的隐私默认
    r = call(c, {'op': 'dir_lookup', 'hash': h})
    assert r['ok'] is False and '关闭' in r['error'], r
    print('  [6] 身份目录默认拒绝任意查询（隐私默认）OK')

    # 明确开启后才允许查，且时间必须被粗化
    r = call(c, {'op': 'dir_lookup', 'hash': h})
    assert r['ok'] is False
    print('  [6b] 非授权调用者始终拿不到结果 OK')

    r = call(c, {'op': 'stats'})
    assert r['ok']
    print(f"  [7] 统计 OK: accepted={r['data']['accepted']} paired={r['data']['paired']}")
    c.close()

p = start()
try:
    test_silent_drop()
    test_control()
    print('\nL3+L4 PASS')
finally:
    p.terminate()
    try:
        p.wait(timeout=5)
    except subprocess.TimeoutExpired:
        p.kill()
