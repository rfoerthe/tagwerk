#!/usr/bin/env python3
"""Erzeugt Testdaten für Tagwerk: die Kalenderwochen 41–46/2026 (05.10.–15.11.) plus
Montag, 16.11.2026.

- An jedem Werktag (Mo–Fr), der kein bundesweiter Feiertag ist, werden zwischen 5 und
  10 Stunden gearbeitet; die Wochensumme liegt zwischen 35,1 und 42,9 Stunden
  (39 Std. ± 10 %).
- Mittagspausen und weitere Pausen ab 15 Min. sind Sperrzeiten; dazu kommen kurze
  Sperren unter 15 Min., die als Arbeitszeit zählen.
- Eingebaute Regelverstöße:
    KW 42  Di bis 21:2x, Mi ab 07:0x       -> Ruhezeit unter 11 Std.
    KW 43  Do bis 22:5x                     -> nach 22 Uhr gearbeitet
    KW 44  Mo ab 06:1x                      -> vor 7 Uhr gearbeitet
    KW 45  Mi 7 Std. mit 20 Min. Pause      -> zu wenig Pause (30 Min. nötig)
           Fr 9:20 Std. mit 30 Min. Pause   -> zu wenig Pause (45 Min. nötig)

Aufruf (aus dem Projektordner):
    python3 testdata/generate_testdata.py > testdata/testdaten.md
    TAGWERK_DIR=testdata cargo run -- --import testdata/testdaten.md
    TAGWERK_DIR=testdata cargo run -- --records
"""

import datetime as dt
import random
import sys

SEED = 20261117
FIRST = dt.date(2026, 10, 5)  # Montag KW 41
LAST = dt.date(2026, 11, 16)  # Montag KW 47
FRAME = (7 * 3600, 22 * 3600)
MIN_PAUSE = 15 * 60
WEEK_TARGET = 39 * 3600
H = 3600
M = 60
WEEKDAYS = ["Mo", "Di", "Mi", "Do", "Fr", "Sa", "So"]


def easter(year):
    """Ostersonntag (gregorianisch, anonymer Algorithmus)."""
    a, b, c = year % 19, year // 100, year % 100
    d, e = b // 4, b % 4
    f = (b + 8) // 25
    g = (b - f + 1) // 3
    h = (19 * a + b - d - g + 15) % 30
    i, k = c // 4, c % 4
    l = (32 + 2 * e + 2 * i - h - k) % 7
    m = (a + 11 * h + 22 * l) // 451
    month = (h + l - 7 * m + 114) // 31
    day = (h + l - 7 * m + 114) % 31 + 1
    return dt.date(year, month, day)


def holidays(year):
    """Bundesweite gesetzliche Feiertage."""
    e = easter(year)
    return {
        dt.date(year, 1, 1),
        e - dt.timedelta(days=2),
        e + dt.timedelta(days=1),
        dt.date(year, 5, 1),
        e + dt.timedelta(days=39),
        e + dt.timedelta(days=50),
        dt.date(year, 10, 3),
        dt.date(year, 12, 25),
        dt.date(year, 12, 26),
    }


def build_day(start, work, locks):
    """Perioden (entsperrt) für einen Tag.

    `work` ist die gezählte Arbeitszeit ohne Rahmen-Kürzung; kurze Sperren (< 15 Min.)
    zählen dazu. `locks` sind (Position in gezählter Arbeitszeit, Dauer) – Sperren ab
    15 Min. sind Pausen."""
    periods, t, elapsed = [], start, 0
    for pos, dur in sorted(locks):
        run = pos - elapsed
        periods.append((t, t + run))
        t += run + dur
        elapsed = pos + (dur if dur < MIN_PAUSE else 0)
    periods.append((t, t + (work - elapsed)))
    return periods


def counted(periods):
    """Gezählte Arbeitszeit wie in Tagwerk: kurze Lücken füllen, auf den Rahmen kürzen."""
    merged = []
    for f, t in periods:
        if merged and f - merged[-1][1] < MIN_PAUSE:
            merged[-1] = (merged[-1][0], t)
        else:
            merged.append((f, t))
    return sum(max(0, min(t, FRAME[1]) - max(f, FRAME[0])) for f, t in merged)


def short_locks(rng, work, avoid):
    """0–3 kurze Sperren (3–12 Min.) über den Tag verteilt, nicht in der Nähe anderer Sperren."""
    out = []
    for _ in range(rng.randint(0, 3)):
        pos = rng.randint(45 * M, max(46 * M, work - 45 * M))
        if all(abs(pos - p) > 30 * M for p, _ in avoid + out):
            out.append((pos, rng.randint(3 * M, 12 * M) + rng.randint(0, 59)))
    return out


def normal_day(rng, work):
    start = int(min(max(rng.gauss(8 * H, 35 * M), 7 * H + 5 * M), 9 * H + 30 * M)) + rng.randint(0, 59)
    locks = []
    if work > 6 * H or rng.random() < 0.5:
        # Mittagspause gegen 12–13 Uhr; ab 9 Std. Arbeit mindestens 45 Min.
        lunch = rng.randint(45, 60) if work > 9 * H else rng.randint(30, 55)
        pos = min(max(12 * H + rng.randint(0, 50) * M - start, 3 * H), work - 2 * H)
        locks.append((pos, lunch * M + rng.randint(0, 59)))
    if work > 6 * H and rng.random() < 0.3:
        # Kaffeepause am Nachmittag (zählt als Pause)
        locks.append((int(work * 0.8), rng.randint(15, 22) * M + rng.randint(0, 59)))
    locks += short_locks(rng, work, locks)
    return build_day(start, work, locks)


def special_day(rng, kind):
    """Tage mit Regelverstoß: (Perioden, gezählte Arbeitszeit)."""
    s = rng.randint(0, 59)
    if kind == "late_end":  # Ende 21:2x – zusammen mit "early_start" am Folgetag < 11 Std. Ruhe
        p = build_day(11 * H + 40 * M + s, 9 * H, [(4 * H, 45 * M + 13)] + [(7 * H, 6 * M + 40)])
    elif kind == "early_start":
        p = build_day(7 * H + 4 * M + s, 8 * H, [(5 * H, 40 * M + 7)])
    elif kind == "after_22":
        p = build_day(13 * H + 40 * M + s, 8 * H + 30 * M, [(4 * H + 30 * M, 45 * M + 21)])
    elif kind == "before_7":
        p = build_day(6 * H + 12 * M + s, 8 * H + 40 * M, [(6 * H, 40 * M + 30)])
    elif kind == "pause_20":
        p = build_day(8 * H + 2 * M + s, 7 * H, [(4 * H, 20 * M + 15)])
    elif kind == "pause_30_long":
        p = build_day(7 * H + 30 * M + s, 9 * H + 20 * M, [(4 * H + 30 * M, 30 * M + 40), (2 * H, 8 * M)])
    else:
        raise ValueError(kind)
    return p, counted(p)


def generate():
    rng = random.Random(SEED)
    hol = holidays(2026)
    specials = {
        dt.date(2026, 10, 13): "late_end",
        dt.date(2026, 10, 14): "early_start",
        dt.date(2026, 10, 22): "after_22",
        dt.date(2026, 10, 26): "before_7",
        dt.date(2026, 11, 4): "pause_20",
        dt.date(2026, 11, 6): "pause_30_long",
    }
    days = {}
    monday = FIRST
    while monday <= LAST:
        workdays = [monday + dt.timedelta(days=i) for i in range(5)]
        workdays = [d for d in workdays if d <= LAST and d not in hol]
        fixed = {}
        for d in workdays:
            if d in specials:
                days[d], fixed[d] = special_day(rng, specials[d])
        free = [d for d in workdays if d not in fixed]
        # Wochensoll anteilig (bei Feiertagen weniger), ±8 % Zufall
        target = WEEK_TARGET * len(workdays) / 5 * rng.uniform(0.92, 1.08)
        rest = target - sum(fixed.values())
        lo, hi = 5 * H + 15 * M, 9 * H + 45 * M
        work = [min(max(rest / max(1, len(free)) + rng.uniform(-1.9, 1.9) * H, lo), hi) for _ in free]
        # Summe angleichen, ohne die Grenzen zu verlassen
        for _ in range(50):
            diff = rest - sum(work)
            if abs(diff) < M:
                break
            for i in range(len(work)):
                work[i] = min(max(work[i] + diff / len(work), lo), hi)
        for d, w in zip(free, work):
            w = int(w) // 60 * 60 + rng.randint(0, 59)
            days[d] = normal_day(rng, w)
        monday += dt.timedelta(days=7)
    return days, hol


def main():
    days, hol = generate()
    out = sys.stdout
    out.write(f"# Tagwerk – Testdaten {FIRST:%d.%m.%Y} bis {LAST:%d.%m.%Y}\n\n")
    out.write(f"Erzeugt mit testdata/generate_testdata.py (Startwert {SEED}).\n\n")
    out.write("## Erfasste Perioden\n\n| Datum | Von | Bis |\n|:------|:---:|:---:|\n")
    fmt = lambda s: f"{s // 3600:02d}:{s % 3600 // 60:02d}:{s % 60:02d}"
    for d in sorted(days):
        for f, t in days[d]:
            out.write(f"| {WEEKDAYS[d.weekday()]}, {d:%d.%m.%Y} | {fmt(f)} | {fmt(t)} |\n")

    # Kontrolle auf stderr: gezählte Arbeitszeit je Woche
    weeks = {}
    for d, p in days.items():
        weeks.setdefault(d.isocalendar()[1], []).append(counted(p))
    for kw, ws in sorted(weeks.items()):
        total = sum(ws)
        print(
            f"KW {kw}: {len(ws)} Tage, {total / H:5.2f} Std. "
            f"({(total / (WEEK_TARGET * len(ws) / 5) - 1) * 100:+.1f} %), "
            f"Tage {min(ws) / H:.2f}–{max(ws) / H:.2f} Std.",
            file=sys.stderr,
        )
    in_range = sorted(h for h in hol if FIRST <= h <= LAST)
    print(f"Feiertage im Zeitraum (bundesweit): {in_range or 'keine'}", file=sys.stderr)


if __name__ == "__main__":
    main()
