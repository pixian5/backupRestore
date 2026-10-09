#!/usr/bin/env python3
"""Parallels 验收入口：按 GUID 定位、串行快照轮换、部署校验和压缩证据。"""
import argparse
import base64
import datetime as dt
import hashlib
import json
import re
import subprocess
import sys
import zipfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
VM = 'Windows 11'


def command(args, check=True):
    result = subprocess.run(args, cwd=ROOT, capture_output=True, text=True, errors='replace')
    if check and result.returncode:
        raise RuntimeError(f'{args[0]} 退出 {result.returncode}: {result.stdout[-2000:]} {result.stderr[-1000:]}')
    return result


def guest(script, dispatch=False):
    script = "[Console]::OutputEncoding=[Text.UTF8Encoding]::new();$ErrorActionPreference='Stop';$ProgressPreference='SilentlyContinue';" + script
    encoded = base64.b64encode(script.encode('utf-16le')).decode()
    result = command(['prlctl', 'exec', VM, 'powershell', '-NoProfile', '-ExecutionPolicy', 'Bypass', '-EncodedCommand', encoded], check=False)
    if result.returncode:
        if dispatch and result.returncode == 255 and re.search(r'^RESUME_DISPATCHED \d+', result.stdout, re.M):
            return result.stdout + '\nGUEST_CHANNEL_INTERRUPTED_AFTER_DISPATCH; verify task state separately\n'
        raise RuntimeError(f'客体命令退出 {result.returncode}: {result.stdout[-1500:]} {result.stderr[-1500:]}')
    return result.stdout


def snapshot(evidence, purpose):
    snapshots = json.loads(command(['prlctl', 'snapshot-list', VM, '-j']).stdout)
    # 只轮换项目命名的快照；未知来源快照使操作停止，避免误删用户数据。
    while len(snapshots) >= 2:
        key = min(snapshots, key=lambda k: snapshots[k]['date'])
        old = snapshots[key]
        if not re.fullmatch(r'\d{8}-\d{6}-re\d+-[a-z0-9-]+', old['name']):
            raise RuntimeError(f"快照不属于本项目，停止轮换：{old['name']}")
        result = command(['prlctl', 'snapshot-delete', VM, '-i', key])
        (evidence / f'snapshot-delete-{key.strip(chr(123)+chr(125))}.txt').write_text(result.stdout)
        snapshots = json.loads(command(['prlctl', 'snapshot-list', VM, '-j']).stdout)
    result = command(['bash', 'tools/vm-snapshot.sh', purpose])
    match = re.search(r'VERIFIED_SNAPSHOT=(\{[0-9a-f-]+\})', result.stdout)
    if not match:
        raise RuntimeError('缺少已核验快照标记')
    record = {'id': match[1], 'purpose': purpose, 'time': dt.datetime.now().astimezone().isoformat()}
    (evidence / 'snapshot.json').write_text(json.dumps(record, ensure_ascii=False, indent=2))
    (evidence / f"snapshot-{match[1][1:-1]}.json").write_text(json.dumps(record, ensure_ascii=False, indent=2))
    print(json.dumps(record, ensure_ascii=False))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('action', choices=['snapshot', 'prepare', 'status', 'retry', 'resume', 'verify', 'collect', 'capture', 'cut', 'pack'])
    parser.add_argument('--case', required=True)
    parser.add_argument('--run', default='re219-reliability-20261009')
    parser.add_argument('--fault', default='acceptance:apply:error')
    parser.add_argument('--boot', action='store_true')
    parser.add_argument('--target-guid', default='{950bd694-9a12-43e5-ad52-98d04672091c}')
    parser.add_argument('--system-image', action='store_true')
    parser.add_argument('--checkpoint')
    args = parser.parse_args()
    if not re.fullmatch('[a-z0-9-]+', args.case) or not re.fullmatch('[a-z0-9-]+', args.run):
        parser.error('名称只允许小写字母、数字和连字符')
    evidence = ROOT / '.test-artifacts' / args.run / args.case
    evidence.mkdir(parents=True, exist_ok=True)
    if args.action == 'snapshot':
        snapshot(evidence, 're219-' + args.case)
        return
    if args.action in ('capture', 'cut'):
        if not args.checkpoint or not re.fullmatch('[a-z0-9-]+', args.checkpoint):
            parser.error('截图或断电必须指定 --checkpoint')
        picture = evidence / (args.checkpoint + '.jpg')
        if args.action == 'capture':
            raw = picture.with_suffix('.png')
            command(['prlctl', 'capture', VM, '--file', str(raw)])
            command(['sips', '-Z', '1280', '-s', 'format', 'jpeg', str(raw), '--out', str(picture)])
            raw.unlink()
            print(picture)
            return
        if not picture.is_file():
            raise RuntimeError('必须先读取对应检查点截图再断电')
        # 仅在操作者已读取客体检查点截图后调用。记录实际强制断电时间和结果。
        (evidence / f'power-cut-{args.checkpoint}.json').write_text(json.dumps({'time':dt.datetime.now().astimezone().isoformat(), 'checkpoint':args.checkpoint, 'screenshotSha256':hashlib.sha256(picture.read_bytes()).hexdigest()}))
        result = command(['prlctl', 'stop', VM, '--kill'])
        (evidence / f'power-cut-{args.checkpoint}.txt').write_text(result.stdout)
        print(result.stdout)
        print(command(['prlctl', 'start', VM]).stdout)
        return
    if args.action == 'pack':
        archive = evidence.parent / (args.run + '-evidence.zip')
        with zipfile.ZipFile(archive, 'w', zipfile.ZIP_DEFLATED) as out:
            for path in evidence.parent.rglob('*'):
                if path.is_file() and path.suffix.lower() in {'.json', '.txt', '.log', '.out', '.err', '.jpg', '.md'} and path.stat().st_size < 5_000_000:
                    out.write(path, path.relative_to(evidence.parent))
        with zipfile.ZipFile(archive) as out:
            assert out.testzip() is None
        print(archive)
        return
    share = '\\\\Mac\\backupRestore'
    config_file = evidence / 'config.json'
    if args.action == 'prepare':
        proof = json.loads((evidence / 'snapshot.json').read_text())
        current = json.loads(command(['prlctl', 'snapshot-list', VM, '-j']).stdout)
        if proof['id'] not in current:
            raise RuntimeError('本次快照已不存在，拒绝部署启动事务')
        exe = ROOT / 'target/aarch64-pc-windows-msvc/release/BackupRestore.exe'
        config = {'case': args.case, 'fault': args.fault, 'boot': args.boot, 'systemImage': args.system_image,
                  'targetGuid': args.target_guid, 'workspaceGuid': '{0e68475f-4890-43f3-ab85-75e8339116ac}',
                  'imageGuid': '{f0753766-30a4-410e-944f-38d139113634}',
                  'rootRelative': f'BRRE-219-20261009\\{args.case}', 'exeSha256': hashlib.sha256(exe.read_bytes()).hexdigest(),
                  'evidence': share + '\\' + str(evidence.relative_to(ROOT)).replace('/', '\\')}
        config_file.write_text(json.dumps(config), encoding='utf-8')
    config_share = share + '\\' + str(config_file.relative_to(ROOT)).replace('/', '\\')
    output = guest(f"& '{share}\\tools\\acceptance\\case.ps1' -Action '{args.action}' -Config '{config_share}'", dispatch=args.action == 'resume')
    (evidence / (args.action + '-' + dt.datetime.now().strftime('%H%M%S') + '.txt')).write_text(output)
    print(output[-5000:])


if __name__ == '__main__':
    main()
