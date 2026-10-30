#!/usr/bin/env python3
import argparse
import json
import os
import re
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from analyze import describe, entries, fmt, nodes_of, num, signers, var  # noqa: E402

VIEWER = os.path.join(os.path.dirname(os.path.abspath(__file__)), 'viewer.html')
MODELS = os.path.join(os.path.dirname(os.path.abspath(__file__)), '..', 'models.qnt')


def model_params(model):
    """committee_size and FAULTY of a model from models.qnt, None if not found."""
    try:
        text = open(MODELS).read()
    except OSError:
        return None
    m = re.search(r'module\s+' + re.escape(model) + r'\s*\{[^}]*committee_size\s*=\s*(\d+),\s*FAULTY\s*=\s*Set\(([^)]*)\)', text)
    if not m:
        return None
    return int(m.group(1)), sorted(int(x) for x in m.group(2).split(',') if x.strip())


def num_set(v):
    return sorted(num(x) for x in v['#set'])


def option(v):
    return v['value'] if v['tag'] == 'Some' else None


def qc(v):
    return {'block': num(v['block']), 'view': num(v['view']), 'signers': signers(v)}


def block(v):
    return {'hash': num(v['hash']), 'parent': num(v['parent']), 'height': num(v['height']), 'justify': qc(v['justify'])}


def node_state(n, sync):
    d, kv = n['driver'], n['kv']
    pm, c = d['pacemaker'], d['consensus']
    lock_timer = option(n['lock_timer'])
    state = {
        'view': num(pm['cur_view']),
        'leader': num(pm['cur_leader']),
        'proposed': pm['proposed'],
        'high_qc': qc(pm['high_qc']),
        'lock': qc(c['qc_lock']),
        'voted_view': num(c['voted_view']),
        'exec_height': num(c['exec_height']),
        'exec_hash': num(c['exec_hash']),
        'committed': [num(b['hash']) for b in kv['committed_branch']],
        'stored': sorted(num(k) for k, _ in entries(kv['block_by_hash'])),
        'sent_wish': num_set(pm['sent_wish']),
        'statuses': num_set(pm['statuses']),
        'relayed_tc': num_set(pm['relayed_tc']),
        'pending': {str(num(k)): [fmt(x) for x in v] for k, v in entries(d['pending']) if v},
        'votes': fmt(d['collector']['generic_collectors']),
        'wishes': fmt(d['collector']['timeout_collectors']),
        'queue': [fmt(x) for x in n['queue']],
        'view_timers': num_set(n['view_timers']),
        'lock_timer': num(lock_timer) if lock_timer is not None else None,
    }
    if sync is not None:
        state['sync'] = {
            'fetches': fmt(sync['fetches']),
            'buffered': sorted(num(k) for k, _ in entries(sync['buffered'])),
            'parked': sorted(num(k) for k, _ in entries(sync['parked'])),
            'delivered': num(sync['delivered']),
            'retries': num(sync['retries']),
            'rejected': num(sync['rejected']),
        }
    return state


def msg_kind(msg):
    return msg['tag'].removesuffix('Msg')


def picks_of(state):
    picks = state.get('mbt::nondetPicks', {})
    return {k: option(v) for k, v in picks.items() if v['tag'] == 'Some'}


class Messages:
    """Matches sends and receives: inboxes are FIFO lists, sync_net is a set."""

    def __init__(self):
        self.all = []
        self.inbox = {}
        self.sync = {}

    def new(self, step, sender, receiver, kind, text, channel):
        m = {'id': len(self.all), 'from': sender, 'to': receiver, 'kind': kind, 'text': text,
             'channel': channel, 'send': step, 'recv': None}
        self.all.append(m)
        return m['id']

    def net_step(self, step, actor, prev, cur, warnings):
        for r in sorted(cur):
            old, new = prev.get(r, []), cur[r]
            ids = self.inbox.setdefault(r, [])
            k = 1 if actor == r and old and new[:len(old) - 1] == old[1:] and new[:len(old)] != old else 0
            if new[:len(old) - k] != old[k:]:
                warnings.append(f'step {step}: inbox of {r} changed unexpectedly, matching restarted')
                ids.clear()
                ids.extend(self.new(step, p_sender(p), r, msg_kind(p['msg']), fmt(p['msg']), 'net') for p in new)
                continue
            if k:
                self.all[ids.pop(0)]['recv'] = step
            for p in new[len(old) - k:]:
                ids.append(self.new(step, p_sender(p), r, msg_kind(p['msg']), fmt(p['msg']), 'net'))

    def sync_step(self, step, prev, cur):
        pk = {json.dumps(p, sort_keys=True): p for p in prev}
        ck = {json.dumps(p, sort_keys=True): p for p in cur}
        for key in pk.keys() - ck.keys():
            if key in self.sync:
                self.all[self.sync.pop(key)]['recv'] = step
        for key in sorted(ck.keys() - pk.keys()):
            p = ck[key]
            self.sync[key] = self.new(step, num(p['from']), num(p['to']), p['msg']['tag'], fmt(p['msg']), 'sync')


def p_sender(p):
    return num(p['sender'])


def export(trace, source):
    states = trace['states']
    model = next((v.split('::')[0] for v in trace['vars'] if not v.startswith('mbt::')), '?')
    running = sorted(nodes_of(states[0]))
    blocks, qcs, warnings = {}, {}, []
    msgs = Messages()
    steps = []
    hosts = set(running)
    for i, s in enumerate(states):
        for b in (var(s, 'minted') or {'#set': []})['#set']:
            h = num(b['hash'])
            if h not in blocks:
                blocks[h] = dict(block(b), minted=i)
        for q in (var(s, 'all_qcs') or {'#set': []})['#set']:
            key = (num(q['view']), num(q['block']))
            if key not in qcs:
                qcs[key] = dict(qc(q), step=i)
        nodes = nodes_of(s)
        syncs = {num(k): v for k, v in entries(var(s, 'syncs'))} if var(s, 'syncs') else {}
        action = s.get('mbt::actionTaken', 'init' if i == 0 else '?')
        picks = picks_of(s)
        actor = num(picks['id']) if 'id' in picks else None
        step = {
            'action': action,
            'actor': actor,
            'label': describe(states[i - 1], s) if i > 0 else 'initial state',
            'nodes': {str(k): node_state(v, syncs.get(k)) for k, v in nodes.items()},
            'inbox': {str(num(k)): [fmt(p) for p in v] for k, v in entries(var(s, 'net')['inbox'])},
        }
        if i > 0:
            prev = states[i - 1]
            pin = {num(k): v for k, v in entries(var(prev, 'net')['inbox'])}
            cin = {num(k): v for k, v in entries(var(s, 'net')['inbox'])}
            msgs.net_step(i, actor if action == 'apply_step' else None, pin, cin, warnings)
            if var(s, 'sync_net') is not None:
                msgs.sync_step(i, var(prev, 'sync_net')['#set'], var(s, 'sync_net')['#set'])
            if action == 'deliver_faulty' and 'packet' in picks:
                p = picks['packet']
                m = msgs.new(i, p_sender(p), actor, msg_kind(p['msg']), fmt(p['msg']), 'faulty')
                msgs.all[m]['recv'] = i
            if action == 'deliver_faulty_sync' and 'from' in picks:
                actor = num(picks['from'])
                step['actor'] = actor
        steps.append(step)
    params = model_params(model)
    committee, faulty = params if params else (None, [])
    hosts.update(faulty)
    for m in msgs.all:
        hosts.update((m['from'], m['to']))
    tags(steps, qcs)
    return {
        'meta': {'model': model, 'source': source, 'steps': len(states) - 1, 'running': running,
                 'hosts': sorted(hosts), 'faulty': sorted(set(faulty) | (hosts - set(running))),
                 'committee_size': committee or len(hosts), 'warnings': warnings},
        'blocks': sorted(blocks.values(), key=lambda b: b['hash']),
        'qcs': sorted(qcs.values(), key=lambda q: (q['step'], q['view'])),
        'messages': msgs.all,
        'steps': steps,
    }


def tags(steps, qcs):
    by_step = {}
    for q in qcs.values():
        by_step.setdefault(q['step'], []).append(q)
    for i, s in enumerate(steps):
        t = []
        if i in by_step and i > 0:
            t.append('qc')
        prev = steps[i - 1]['nodes'] if i > 0 else None
        if prev is not None:
            for k, n in s['nodes'].items():
                if n['exec_height'] != prev[k]['exec_height']:
                    t.append('commit')
                if n['view'] != prev[k]['view']:
                    t.append('view')
        if s['action'].startswith('fire_'):
            t.append('timer')
        if s['action'].startswith('deliver_faulty'):
            t.append('faulty')
        if s['action'].startswith('sync'):
            t.append('sync')
        if 'dropped' in s['label']:
            t.append('drop')
        s['tags'] = sorted(set(t))


def shiviz(data):
    """ShiViz log: an event line, then `host {vector clock}`. Default ShiViz regex parses it."""
    clocks = {h: {} for h in data['meta']['hosts']}
    sent = {}
    by_send, by_recv = {}, {}
    for m in data['messages']:
        by_send.setdefault(m['send'], []).append(m)
        if m['recv'] is not None:
            by_recv.setdefault(m['recv'], []).append(m)
    lines = []

    def tick(host, text, merge=()):
        c = clocks[host]
        for other in merge:
            for h, t in other.items():
                c[h] = max(c.get(h, 0), t)
        c[f'n{host}'] = c.get(f'n{host}', 0) + 1
        lines.append(text.replace('\n', ' '))
        lines.append(f'n{host} ' + json.dumps(c))
        return dict(c)

    for i, s in enumerate(data['steps']):
        if i == 0:
            continue
        outs = by_send.get(i, [])
        for m in outs:
            if m['from'] != s['actor'] and m['channel'] == 'faulty':
                sent[m['id']] = tick(m['from'], f'[{i}] send {m["text"]} to {m["to"]}')
        ins = [sent[m['id']] for m in by_recv.get(i, []) if m['id'] in sent]
        if s['actor'] is not None:
            clock = tick(s['actor'], f'[{i}] {s["label"]}', ins)
            for m in outs:
                if m['from'] == s['actor']:
                    sent[m['id']] = clock
    return '\n'.join(lines) + '\n'


def dot(data, step):
    s = data['steps'][step]
    certified = {q['block'] for q in data['qcs'] if q['step'] <= step}
    lines = ['digraph blocks {', '  rankdir=LR;', '  node [shape=box, fontname="monospace"];']
    for b in data['blocks']:
        if b['minted'] > step:
            continue
        marks = []
        for k, n in sorted(s['nodes'].items()):
            m = ''
            if b['hash'] in n['committed'] or b['hash'] == n['exec_hash']:
                m += 'C'
            if n['lock']['block'] == b['hash']:
                m += 'L'
            if n['high_qc']['block'] == b['hash']:
                m += 'H'
            if m:
                marks.append(f'{k}:{m}')
        style = 'style=bold' if b['hash'] in certified else 'style=dashed'
        label = f'b{b["hash"]} h{b["height"]}' + (r'\n' + ' '.join(marks) if marks else '')
        lines.append(f'  b{b["hash"]} [label="{label}", {style}];')
        if b['parent'] >= 0:
            lines.append(f'  b{b["parent"]} -> b{b["hash"]};')
        if b['justify']['block'] not in (b['parent'], b['hash']):
            lines.append(f'  b{b["justify"]["block"]} -> b{b["hash"]} [style=dotted, label="justify"];')
    lines.append('}')
    return '\n'.join(lines) + '\n'


def main():
    parser = argparse.ArgumentParser(description='Export a Quint ITF trace for visualization.')
    parser.add_argument('trace')
    parser.add_argument('--html', help='self-contained viewer with the trace inlined')
    parser.add_argument('--json', help='trace in the viewer format')
    parser.add_argument('--shiviz', help='log for https://bestchai.bitbucket.io/shiviz/')
    parser.add_argument('--dot', help='block tree in Graphviz format')
    parser.add_argument('--step', type=int, help='step for --dot (default: last)')
    args = parser.parse_args()
    trace = json.load(open(args.trace))
    # A viewer JSON (e.g. from the Rust simulator) is taken as is.
    data = trace if 'steps' in trace and 'meta' in trace else export(trace, os.path.basename(args.trace))
    for w in data['meta']['warnings']:
        print('warning:', w, file=sys.stderr)
    if args.json:
        with open(args.json, 'w') as f:
            json.dump(data, f, separators=(',', ':'))
    if args.html:
        payload = json.dumps(data, separators=(',', ':')).replace('</', '<\\/')
        with open(VIEWER) as f:
            page = f.read()
        with open(args.html, 'w') as f:
            f.write(page.replace('const EMBEDDED = null;', f'const EMBEDDED = {payload};', 1))
    if args.shiviz:
        with open(args.shiviz, 'w') as f:
            f.write(shiviz(data))
    if args.dot:
        with open(args.dot, 'w') as f:
            f.write(dot(data, data['meta']['steps'] if args.step is None else args.step))


if __name__ == '__main__':
    main()
