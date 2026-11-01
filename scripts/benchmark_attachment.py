#!/usr/bin/env python3
"""Measure one attachment extraction, optionally in an isolated cgroup v2.

Use a fresh systemd-run scope/service per invocation for an aggregate memory
measurement. cgroup memory.peak is cumulative for the whole group's lifetime.
Without isolation, its values may also include unrelated processes.
"""

import argparse
import datetime
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import time


METRICS = (
    'memory.current', 'memory.peak', 'memory.max', 'memory.swap.current',
    'memory.swap.max', 'memory.events', 'cpu.max', 'cpu.stat',
)


def cgroup_path():
    try:
        for line in Path('/proc/self/cgroup').read_text().splitlines():
            if line.startswith('0:'):
                return Path('/sys/fs/cgroup') / line.split(':', 2)[2].lstrip('/')
    except OSError:
        pass
    return None


def metrics(group):
    result = {}
    if group is None:
        return result
    for name in METRICS:
        try:
            text = (group / name).read_text().strip()
        except OSError:
            continue
        if name in ('memory.events', 'cpu.stat'):
            result[name] = {key: int(value) for key, value in (line.split() for line in text.splitlines())}
        else:
            result[name] = int(text) if text.isdecimal() else text
    return result


def cpu_model():
    try:
        for line in Path('/proc/cpuinfo').read_text().splitlines():
            if line.startswith('model name'):
                return line.split(':', 1)[1].strip()
    except OSError:
        pass
    return None


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('input', type=Path, help='One attachment to measure')
    parser.add_argument('--name', help='Original filename, if the input has another extension')
    parser.add_argument('--extractor', type=Path, default=Path(__file__).resolve().with_name('extract_attachment.py'))
    parser.add_argument('--max-memory-mb', type=int, default=384)
    parser.add_argument('--timeout-seconds', type=int, default=3600)
    parser.add_argument('--max-pages', type=int, default=200)
    parser.add_argument('--max-output-chars', type=int, default=2000000)
    parser.add_argument('--contains', help='Optional control phrase expected in extracted pages')
    parser.add_argument('--output', type=Path, help='Also save the JSON report to this file')
    args = parser.parse_args()
    if not args.input.is_file():
        parser.error('Input file does not exist')
    group = cgroup_path()
    before = metrics(group)
    sampled_peak = before.get('memory.current', 0)
    command = [sys.executable, str(args.extractor.resolve()), '--input', str(args.input.resolve()),
               '--name', args.name or args.input.name, '--max-memory-mb', str(args.max_memory_mb),
               '--timeout-seconds', str(args.timeout_seconds), '--max-pages', str(args.max_pages),
               '--max-output-chars', str(args.max_output_chars)]
    started_at = datetime.datetime.now(datetime.timezone.utc).isoformat()
    started = time.monotonic()
    with tempfile.TemporaryFile() as output, tempfile.TemporaryFile() as errors:
        process = subprocess.Popen(command, stdout=output, stderr=errors, start_new_session=True)
        try:
            while process.poll() is None:
                current = metrics(group).get('memory.current')
                if isinstance(current, int):
                    sampled_peak = max(sampled_peak, current)
                if time.monotonic() - started > args.timeout_seconds + 10:
                    raise TimeoutError('Extractor did not stop after its deadline')
                time.sleep(0.05)
        except BaseException:
            # The extractor handles SIGTERM by terminating its child process group.
            process.terminate()
            try:
                process.wait(timeout=3)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait()
            raise
        elapsed = time.monotonic() - started
        output.seek(0)
        try:
            extraction = json.loads(output.read(args.max_output_chars * 8 + 1024 * 1024))
        except (ValueError, UnicodeDecodeError):
            extraction = {'status': 'failed', 'error': 'Extractor did not return a valid JSON result'}
    after = metrics(group)
    if isinstance(after.get('memory.current'), int):
        sampled_peak = max(sampled_peak, after['memory.current'])
    events_before = before.get('memory.events', {})
    events_after = after.get('memory.events', {})
    pages = extraction.get('pages', [])
    report = {
        'started_at_utc': started_at,
        'source_file': args.input.name,
        'source_size_bytes': args.input.stat().st_size,
        'source_sha256': hashlib.sha256(args.input.read_bytes()).hexdigest(),
        'cpu_model': cpu_model(),
        'logical_cpus': os.cpu_count(),
        'status': extraction.get('status', 'failed'),
        'error': extraction.get('error'),
        'returncode': process.returncode,
        'method': extraction.get('method'),
        'wall_seconds': round(elapsed, 3),
        'extractor_duration_seconds': extraction.get('duration_seconds'),
        'pages': len(pages),
        'characters': sum(len(page.get('text', '')) for page in pages),
        'max_individual_process_rss_kib': extraction.get('peak_process_rss_kib'),
        'max_individual_process_rss_note': 'Maximum of individual process RSS peaks. This is not total memory of simultaneous processes.',
        'warnings': extraction.get('warnings', []),
        'limits': {'process_address_space_mib': args.max_memory_mb,
                   'timeout_seconds': args.timeout_seconds, 'max_pages': args.max_pages,
                   'max_output_chars': args.max_output_chars},
        'cgroup': {
            'path': str(group) if group else None,
            'note': 'All processes in this cgroup are included. memory.peak is cumulative for its lifetime. Use a new isolated group for each measured file.',
            'before': before,
            'after': after,
            'aggregate_current_sampled_peak_bytes': sampled_peak if 'memory.current' in before else None,
            'sample_interval_seconds': 0.05,
            'event_delta': {key: value - events_before.get(key, 0) for key, value in events_after.items()},
        },
    }
    if args.contains is not None:
        report['control_phrase'] = args.contains
        report['control_phrase_pages'] = sum(args.contains in page.get('text', '') for page in pages)
    rendered = json.dumps(report, ensure_ascii=False, indent=2)
    if args.output:
        args.output.write_text(rendered + '\n', encoding='utf-8')
    print(rendered)
    return 0 if report['status'] == 'ready' else 1


if __name__ == '__main__':
    sys.exit(main())
