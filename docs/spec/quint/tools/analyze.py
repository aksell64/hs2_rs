#!/usr/bin/env python3
import argparse
import json


def var(state, name):
    for key, value in state.items():
        if key.split('::')[-1] == name:
            return value
    return None


def num(v):
    return int(v['#bigint'])


def is_unit(v):
    return isinstance(v, dict) and v.get('#tup') == []


def entries(m):
    return m['#map']


def map_get(m, key):
    for k, v in entries(m):
        if num(k) == key:
            return v
    return None


def fmt(v):
    if isinstance(v, bool):
        return 'true' if v else 'false'
    if isinstance(v, (int, str)):
        return str(v)
    if isinstance(v, list):
        return '[' + ', '.join(fmt(x) for x in v) + ']'
    if '#bigint' in v:
        return v['#bigint']
    if '#set' in v:
        items = sorted(fmt(x) for x in v['#set'])
        return '{' + ', '.join(items) + '}' if items else '∅'
    if '#map' in v:
        items = [f'{fmt(k)} -> {fmt(x)}' for k, x in v['#map']]
        return '; '.join(items) if items else '∅'
    if '#tup' in v:
        return '(' + ', '.join(fmt(x) for x in v['#tup']) + ')'
    if 'tag' in v and 'value' in v and len(v) == 2:
        return v['tag'] if is_unit(v['value']) else f"{v['tag']}({fmt(v['value'])})"
    keys = set(v)
    if keys == {'block', 'certificate', 'view'}:
        return f"QC(b{fmt(v['block'])}@v{fmt(v['view'])} {signers(v)})"
    if keys == {'certificate', 'view'}:
        return f"TC(v{fmt(v['view'])} {signers(v)})"
    if keys == {'hash', 'height', 'justify', 'parent'}:
        return f"B{fmt(v['hash'])}(h{fmt(v['height'])}, parent {fmt(v['parent'])}, {fmt(v['justify'])})"
    return '{ ' + ', '.join(f'{k}: {fmt(x)}' for k, x in v.items()) + ' }'


def signers(cert):
    sig = cert['certificate']['signature']
    if sig['tag'] != 'QuorumCert':
        return sig['tag']
    return '{' + ','.join(str(num(x)) for x in sorted(sig['value']['#set'], key=num)) + '}'


def nodes_of(state):
    return {num(k): v for k, v in entries(var(state, 'nodes'))}


def inbox_of(state):
    return {num(k): v for k, v in entries(var(state, 'net')['inbox'])}


def faulty_sender(item, node_ids):
    value = item.get('value')
    if isinstance(value, dict) and 'sender' in value:
        return num(value['sender']) not in node_ids
    return False


def popped(old, new):
    return bool(old) and new[:len(old) - 1] == old[1:] and new[:len(old)] != old


def describe(prev, cur):
    pn, cn = nodes_of(prev), nodes_of(cur)
    pi, ci = inbox_of(prev), inbox_of(cur)
    if pn == cn and pi == ci:
        return 'idle step'
    changed = [i for i in cn if cn[i] != pn[i]]
    for i in sorted(cn):
        old, new = pi.get(i, []), ci.get(i, [])
        if popped(old, new) and i not in changed:
            return f'node {i}: took `{fmt(old[0])}` from the network — **dropped, state unchanged**'
    if not changed:
        return 'only the network changed'
    i = changed[0]
    p, c = pn[i], cn[i]
    pq, cq = p['queue'], c['queue']
    fired = [v for v in p['view_timers']['#set'] if v not in c['view_timers']['#set']]
    if fired and p['driver'] == c['driver'] and cq[-1:] == [{'tag': 'ViewTimerFired', 'value': fired[0]}]:
        return f'node {i}: view_timer fired → `{fmt(cq[-1])}`'
    if p['lock_timer']['tag'] == 'Some' and c['lock_timer']['tag'] == 'None' and cq[-1:] == [{'tag': 'LockTimerFired', 'value': p['lock_timer']['value']}]:
        return f'node {i}: lock_timer fired → `{fmt(cq[-1])}`'
    if cq[:len(pq)] == pq and len(cq) == len(pq) + 1 and p['driver'] == c['driver'] and faulty_sender(cq[-1], set(cn)):
        return f'node {i}: faulty node delivered `{fmt(cq[-1])}`'
    old, new = pi.get(i, []), ci.get(i, [])
    took = old[0] if popped(old, new) else None
    head = pq[0] if pq else None
    if took is not None and head is None:
        return f'node {i}: took from the network and processed `{fmt(took)}`'
    if took is not None:
        return f'node {i}: queued `{fmt(took)}` from the network, processed `{fmt(head)}`'
    if head is not None:
        return f'node {i}: processed `{fmt(head)}`'
    return f'node {i}: state changed'


ROWS = [
    ('pacemaker', ['cur_view', 'cur_leader', 'high_qc', 'proposed', 'statuses', 'sent_wish', 'relayed_tc', 'id']),
    ('consensus', ['voted_view', 'qc_lock', 'exec_height', 'exec_hash']),
    ('driver', ['pending']),
    ('node', ['view_timers', 'lock_timer', 'queue']),
]


def field(node, comp, name):
    if comp == 'node':
        return node.get(name)
    if comp == 'driver':
        return node['driver'].get(name)
    return node['driver'][comp].get(name)


def cell(value, comp, name):
    if value is None:
        return '—'
    if comp == 'node' and name == 'queue':
        return '<br>'.join(fmt(x) for x in value) if value else '—'
    if isinstance(value, dict) and '#map' in value:
        items = [f'{fmt(k)} -> {fmt(x)}' for k, x in value['#map']]
        return '<br>'.join(items) if items else '∅'
    return fmt(value)


def qcs_of(state):
    qcs = var(state, 'all_qcs')
    if qcs is None:
        return set()
    return {(num(q['view']), num(q['block'])) for q in qcs['#set']}


def summary(states):
    lines = ['## Summary', '']
    prev_qcs, prev_exec = None, None
    events = []
    for n, s in enumerate(states):
        qcs = qcs_of(s)
        if prev_qcs is not None and qcs != prev_qcs:
            new = sorted(qcs - prev_qcs)
            events.append(f'| {n} | new QC: ' + ', '.join(f'QC(b{b}@v{v})' for v, b in new) + ' |')
        execs = {i: num(x['driver']['consensus']['exec_height']) for i, x in nodes_of(s).items()}
        if prev_exec is not None and execs != prev_exec:
            events.append(f'| {n} | commit: exec_height ' + ' / '.join(str(execs[i]) for i in sorted(execs)) + ' |')
        prev_qcs, prev_exec = qcs, execs
    if events:
        lines += ['| step | event |', '|---|---|'] + events + ['']
    else:
        lines += ['No new QCs or commits in this trace.', '']
    last = nodes_of(states[-1])
    lines.append('End of trace: view ' + ' / '.join(fmt(last[i]['driver']['pacemaker']['cur_view']) for i in sorted(last)) + '.')
    lines.append('')
    for i in sorted(last):
        chain = [fmt(b['hash']) for b in last[i]['kv']['committed_branch']]
        lines.append(f'- node {i}, committed {len(chain)}: ' + (', '.join(chain) if chain else '—'))
    lines.append('')
    return lines


def steps(states, lo, hi):
    lines = []
    for n in range(lo, hi + 1):
        cur = states[n]
        nodes = nodes_of(cur)
        ids = sorted(nodes)
        prev = nodes_of(states[n - 1]) if n > 0 else None
        lines += [f'## Step {n}', '']
        if n > 0:
            lines += ['Action: ' + describe(states[n - 1], cur), '']
        lines.append('| | ' + ' | '.join(f'node {i}' for i in ids) + ' |')
        lines.append('|---|' + '---|' * len(ids))
        for comp, names in ROWS:
            for name in names:
                cells = []
                for i in ids:
                    value = field(nodes[i], comp, name)
                    text = cell(value, comp, name)
                    if prev is not None and value != field(prev[i], comp, name):
                        text = f'**{text}**'
                    cells.append(text)
                lines.append(f'| {comp}.{name} | ' + ' | '.join(cells) + ' |')
        lines += ['', 'Network:', '']
        for i, packets in sorted(inbox_of(cur).items()):
            lines.append(f'- {i} -> ' + (', '.join(fmt(p) for p in packets) if packets else '[]'))
        lines.append('')
    return lines


def main():
    parser = argparse.ArgumentParser(description='Render a Quint ITF trace as markdown.')
    parser.add_argument('trace')
    parser.add_argument('out')
    parser.add_argument('--from', dest='lo', type=int, default=1)
    parser.add_argument('--to', dest='hi', type=int, default=30)
    args = parser.parse_args()
    trace = json.load(open(args.trace))
    states = trace['states']
    model = trace['vars'][0].split('::')[0]
    hi = min(args.hi, len(states) - 1)
    lines = [f'# Trace analysis `{args.trace}`', '',
             f'Model: `{model}`, steps in trace: {len(states) - 1}, detailed steps {args.lo}–{hi}. '
             'Each table shows the state after the step; changed values are in bold.', '']
    lines += summary(states)
    lines += steps(states, args.lo, hi)
    with open(args.out, 'w') as f:
        f.write('\n'.join(lines) + '\n')


if __name__ == '__main__':
    main()
