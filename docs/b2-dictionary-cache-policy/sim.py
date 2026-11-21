"""Simulate eviction policies on a cyclic block trace (A2 Q9 shape): W distinct blocks, same order each pass,
16 shards by index % 16, capacity in blocks per shard. Reports steady-state hit rate (passes 3..10)."""
import random, collections, sys
def run(policy, W, C, passes=10, shards=16, seed=1):
    rng = random.Random(seed); cap = max(1, C // shards); hits = tot = 0
    st = [dict(fifo=collections.OrderedDict(), small=collections.OrderedDict(), main=collections.OrderedDict(),
               ghost=collections.OrderedDict(), keys=[], pos={}) for _ in range(shards)]
    for p in range(passes):
        for b in range(W):
            s = st[b % shards]; count = p >= 2
            if policy in ('fifo', 'lru'):
                d = s['fifo']
                if b in d:
                    hits += count; tot += count
                    if policy == 'lru': d.move_to_end(b)
                    continue
                tot += count
                if len(d) >= cap: d.popitem(last=False)
                d[b] = 1
            elif policy == 'random':
                if b in s['pos']: hits += count; tot += count; continue
                tot += count; keys = s['keys']
                if len(keys) >= cap:
                    i = rng.randrange(len(keys)); old = keys[i]; keys[i] = b; del s['pos'][old]; s['pos'][b] = i
                else: s['pos'][b] = len(keys); keys.append(b)
            elif policy == 's3fifo':
                S, M, G = s['small'], s['main'], s['ghost']; scap = max(1, cap // 10); mcap = cap - scap
                if b in S or b in M:
                    q = S if b in S else M; q[b] = min(q[b] + 1, 3); hits += count; tot += count; continue
                tot += count
                def evict_m():
                    while M:
                        k, f = M.popitem(last=False)
                        if f > 0: M[k] = f - 1
                        else: return
                if b in G:
                    del G[b]
                    if len(M) >= mcap: evict_m()
                    M[b] = 0
                else:
                    if len(S) >= scap:
                        k, f = S.popitem(last=False)
                        if f > 0:
                            if len(M) >= mcap: evict_m()
                            M[k] = 0
                        else:
                            G[k] = 1
                            if len(G) > cap: G.popitem(last=False)
                    S[b] = 0
    return hits / tot
W = 54231
for frac in (0.64, 0.95, 1.27):
    C = int(W * frac)
    print(f"capacity/working-set {frac:.2f}: " + ", ".join(f"{pol}={run(pol, W, C):.3f}" for pol in ('fifo', 'lru', 'random', 's3fifo')))

def run_admit(p_admit, W, C, passes=10, shards=16, seed=1, shift_at=None):
    """Random eviction, but a miss on a full shard is admitted only with probability p_admit.
    shift_at: pass index after which the working set changes entirely (tests adaptation)."""
    rng = random.Random(seed); cap = max(1, C // shards); hits = tot = 0; per_pass = []
    st = [dict(keys=[], pos={}) for _ in range(shards)]
    for p in range(passes):
        ph = pt = 0
        base = W if (shift_at is not None and p >= shift_at) else 0
        for b0 in range(W):
            b = b0 + base; s = st[b % shards]
            if b in s['pos']: ph += 1; pt += 1; continue
            pt += 1; keys = s['keys']
            if len(keys) < cap: s['pos'][b] = len(keys); keys.append(b)
            elif rng.random() < p_admit:
                i = rng.randrange(len(keys)); old = keys[i]; keys[i] = b; del s['pos'][old]; s['pos'][b] = i
        per_pass.append(ph / pt)
    return per_pass
for frac in (0.64, 0.95):
    for pa in (1.0, 0.5, 0.25, 0.1):
        pp = run_admit(pa, W, int(W * frac))
        print(f"cap/ws {frac:.2f} admit p={pa}: steady hit={sum(pp[2:]) / len(pp[2:]):.3f}")
for pa in (0.25, 0.1):
    pp = run_admit(pa, W, int(W * 0.64), passes=12, shift_at=6)
    print(f"shift test p={pa}: hit per pass after working-set change = {[round(x, 2) for x in pp[6:]]}")
