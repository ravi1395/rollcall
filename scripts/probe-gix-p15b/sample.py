import subprocess, sys, time
mode, fx, secs, tick = sys.argv[1], sys.argv[2], float(sys.argv[3]), float(sys.argv[4])
p = subprocess.Popen([sys.argv[5], mode, fx, "30"], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
t0 = time.monotonic(); prev = 0; maxd = 0; peak = 0; cross384 = None; samples = []
while time.monotonic() - t0 < secs and p.poll() is None:
    out = subprocess.run(["/bin/ps", "-o", "rss=", "-p", str(p.pid)], capture_output=True, text=True).stdout.strip()
    if out:
        rss = int(out) // 1024
        d = rss - prev; maxd = max(maxd, d); prev = rss; peak = max(peak, rss)
        samples.append((round(time.monotonic()-t0, 3), rss))
        if cross384 is None and rss > 384: cross384 = round(time.monotonic()-t0, 3)
    time.sleep(tick)
alive = p.poll() is None
if alive: p.kill()
print(f"{mode} {fx.split('/')[-2]}: peak={peak}MiB max_delta_per_tick={maxd}MiB first>384MiB@{cross384}s exited={not alive} rc={p.returncode} n={len(samples)}")
print(" samples:", samples[:6], "...", samples[-4:])
