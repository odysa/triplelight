"""B2 v1 simulation: policies x traces, 16 shards (block % 16), equal-size blocks, capacity in blocks.
Policies: fifo, lru, rand_admit (random eviction, admit p into full shard), refbit_admit (hit sets a reference
bit; eviction samples 8 residents and evicts an unreferenced one, clearing sampled bits; admit p into full shard).
Traces: cyclic (warm-up pass + 2 scored passes, hit rate per position), zipf (s=1.0), hot+scan."""
import random, bisect, itertools
SH = 16
class Shard:
    def __init__(s, policy, cap, p, rng):
        s.policy, s.cap, s.p, s.rng = policy, cap, p, rng
        s.keys, s.pos, s.ref = [], {}, {}
        s.order = {}  # fifo / lru via dict insertion order
    def access(s, b):
        pol = s.policy
        if pol in ('fifo', 'lru'):
            if b in s.order:
                if pol == 'lru':
                    del s.order[b]; s.order[b] = 1
                return True
            if len(s.order) >= s.cap:
                del s.order[next(iter(s.order))]
            s.order[b] = 1
            return False
        if b in s.pos:
            if pol == 'refbit_admit': s.ref[b] = 1
            return True
        if len(s.keys) < s.cap:
            s.pos[b] = len(s.keys); s.keys.append(b); s.ref[b] = 0
            return False
        if s.rng.random() >= s.p: return False
        if pol == 'rand_admit':
            i = s.rng.randrange(len(s.keys))
        else:
            i = None
            for _ in range(8):
                j = s.rng.randrange(len(s.keys)); k = s.keys[j]
                if s.ref[k]: s.ref[k] = 0
                elif i is None: i = j
            if i is None: i = j
        old = s.keys[i]; del s.pos[old]; del s.ref[old]
        s.keys[i] = b; s.pos[b] = i; s.ref[b] = 0
        return False
def make(policy, cap_blocks, p=0.25, seed=1):
    return [Shard(policy, max(1, cap_blocks // SH), p, random.Random(seed * 1000 + i)) for i in range(SH)]
def hit(c, b): return c[b % SH].access(b)
POL = ('fifo', 'lru', 'rand_admit', 'refbit_admit')
def cyclic(W, frac):
    out = {}
    for pol in POL:
        c = make(pol, int(W * frac)); res = []
        for p in range(3):
            res.append(sum(hit(c, b) for b in range(W)) / W)
        out[pol] = res
    return out
def zipf_trace(n_blocks, n, s=1.0, seed=7):
    rng = random.Random(seed)
    w = [1 / (i + 1) ** s for i in range(n_blocks)]; cum = list(itertools.accumulate(w)); tot = cum[-1]
    perm = list(range(n_blocks)); rng.shuffle(perm)
    return [perm[bisect.bisect(cum, rng.random() * tot)] for _ in range(n)]
def hot_scan_trace(hot, scan, n, seed=9):
    rng = random.Random(seed); out = []; sp = 0
    for _ in range(n):
        if rng.random() < 0.5: out.append(rng.randrange(hot))
        else: out.append(hot + sp); sp = (sp + 1) % scan
    return out
def rate(trace, pol, cap, warm):
    c = make(pol, cap); h = 0
    for i, b in enumerate(trace):
        x = hit(c, b)
        if i >= warm: h += x
    return h / (len(trace) - warm)
W = 54231
print("cyclic (pass1 = warm-up, pass2 = position 1, pass3 = position 2):")
for frac in (0.64, 0.95, 1.27):
    r = cyclic(W, frac)
    print(f"  cap/ws {frac}: " + ", ".join(f"{k}={'/'.join(f'{x:.2f}' for x in v)}" for k, v in r.items()))
z = zipf_trace(200_000, 600_000)
print("zipf s=1.0, 200k blocks, cache 20k blocks:", ", ".join(f"{p}={rate(z, p, 20_000, 200_000):.3f}" for p in POL))
hs = hot_scan_trace(2_000, 200_000, 600_000)
print("hot set 2k (50% of accesses) + scan of 200k, cache 20k:", ", ".join(f"{p}={rate(hs, p, 20_000, 200_000):.3f}" for p in POL))
