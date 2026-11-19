#!/usr/bin/env python3
"""Erzeugt Testdaten für Tagwerk als Markdown (Format von „Tagwerk --export“).

Der Zeitraum hängt vom heutigen Tag ab: Er endet gestern und beginnt am Montag sechs
Wochen vor der aktuellen Woche – also sechs volle Kalenderwochen plus die Tage der
laufenden Woche bis gestern. Heute selbst bleibt frei.

- An jedem Werktag (Mo–Fr), der kein bundesweiter Feiertag ist, werden zwischen 5 und
  10 Stunden gearbeitet; die Wochensumme liegt zwischen 35,1 und 42,9 Stunden
  (39 Std. ± 10 %, bei Feiertagen und in der laufenden Woche anteilig).
- Mittagspausen und weitere Pausen ab 15 Min. sind Sperrzeiten; dazu kommen kurze
  Sperren unter 15 Min., die als Arbeitszeit zählen.
- Arbeit ohne Mac (Datenbankschema 3, Tabelle work_blocks) an rund 25 % der Werktage,
  jeweils eine Art pro Tag:
    Meeting        Mac 45–90 Min. gesperrt, als Meeting nachgetragen
    Telefonat      eine Pause (20–35 Min.) als Telefonat gezählt
    Sonstiges      Mac 30–75 Min. gesperrt, ohne Notiz
    Networking     Mac 30–60 Min. gesperrt, als Networking nachgetragen
    hintereinander Meeting und direkt anschließend Telefonat in einer Sperre: zwei
                   aneinandergrenzende Abschnitte (Tagwerk fasst sie nicht zusammen)
    vor Beginn     Telefonat vor dem ersten Entsperren
    nach Ende      Telefonat oder Meeting nach dem letzten Sperren
  Nachgetragene Abschnitte (Dialog bzw. Inspektor) beginnen und enden auf volle Minuten;
  nur die als Telefonat gezählte Pause übernimmt die Sekunden der erfassten Sperre.
- Eingebaute Regelverstöße (Woche 1 = erste Woche des Zeitraums; liegt der Wunschtag auf
  einem Feiertag, rückt der Verstoß auf einen anderen Tag derselben Woche):
    Woche 2  Di bis 21:2x, Mi ab 07:0x       -> Ruhezeit unter 11 Std.
    Woche 3  Do bis 22:5x                     -> nach 22 Uhr gearbeitet
    Woche 4  Mo ab 06:1x                      -> vor 7 Uhr gearbeitet
    Woche 5  Mi 7 Std. mit 20 Min. Pause      -> zu wenig Pause (30 Min. nötig)
             Fr 9:20 Std. mit 30 Min. Pause   -> zu wenig Pause (45 Min. nötig)
- Mehr als 6 Std. ohne Pause an rund 10 % der Werktage, über den Zeitraum verteilt:
  zwischen 6:10 und 8:45 Std. Arbeit am Stück, unterbrochen höchstens von kurzen Sperren
  unter 15 Min. (zählen als Arbeitszeit) -> zu wenig Pause (30 Min. nötig). Diese Tage
  haben keine Arbeit ohne Mac und keinen weiteren Regelverstoß.
  Alle anderen Tage halten Arbeitszeitgesetz und Arbeitszeitrahmen ein; das prüft das
  Skript selbst (wie Tagwerk) und meldet auf stderr eine Übersicht.

Aufruf (aus dem Projektordner; testdata/testdb.sh erledigt das):
    python3 testdata/generate_testdata.py > target/testdata/testdaten.md
    python3 testdata/generate_testdata.py --today 2026-11-18   # anderer Stichtag
"""

import argparse
import datetime as dt
import random
import sys

SEED = 20261117
WEEKS = 6
FRAME = (7 * 3600, 22 * 3600)
MIN_PAUSE = 15 * 60
MIN_REST = 11 * 3600
MAX_WORK = 10 * 3600
WEEK_TARGET = 39 * 3600
WORK_SHARE = 0.25  # Anteil der Werktage mit Arbeit ohne Mac
NO_BREAK_SHARE = 0.10  # Anteil der Werktage mit mehr als 6 Std. Arbeit ohne Pause
H = 3600
M = 60
DAY = 24 * H
EARLIEST = 7 * H + 5 * M  # frühester Beginn normaler Tage
LATEST_END = 20 * H  # spätestes Ende normaler Tage (Ruhezeit bis 7 Uhr)
WORK_RANGE = (5 * H + 15 * M, 9 * H + 45 * M)  # Arbeitszeit normaler Tage
# Arbeitszeit der Tage ohne Pause: über 6 Std., aber unter 9 Std. (sonst wären 45 Min. nötig)
NO_BREAK_RANGE = (6 * H + 10 * M, 8 * H + 45 * M)
NO_BREAK = "no_break"
WEEKDAYS = ["Mo", "Di", "Mi", "Do", "Fr", "Sa", "So"]
KIND_WORK = "Arbeit ohne Mac"

# Regelverstöße: (Woche, Arten an aufeinanderfolgenden Tagen, Wunsch-Wochentage 0=Mo)
SPECIALS = [
    (1, ("late_end", "early_start"), [1, 0, 2, 3]),
    (2, ("after_22",), [3, 2, 1, 0, 4]),
    (3, ("before_7",), [0, 1, 2, 3, 4]),
    (4, ("pause_20",), [2, 1, 3, 0, 4]),
    (4, ("pause_30_long",), [4, 3, 2, 1, 0]),
]
# Erwartete Hinweise je Art (wie Tagwerk sie meldet)
EXPECTED = {
    "early_start": {"Ruhezeit"},
    "after_22": {"außerhalb Rahmen"},
    "before_7": {"außerhalb Rahmen"},
    "pause_20": {"Pause 30 Min."},
    "pause_30_long": {"Pause 45 Min."},
    NO_BREAK: {"Pause 30 Min."},
}
VARIANTS = ["meeting", "call_pause", "other", "networking", "back_to_back", "call_morning", "call_evening"]
# Arten, die in einer Sperre nachgetragen werden: (Dauer in Min. von–bis, Notiz)
LOCKED = {
    "meeting": ((45, 90), "Meeting"),
    "other": ((30, 75), ""),
    "networking": ((30, 60), "Networking"),
    "back_to_back": ((50, 90), "Meeting"),  # wird in Meeting + Telefonat geteilt
}
VARIANT_NAMES = {
    "meeting": "Meeting (gesperrt)",
    "call_pause": "Pause als Telefonat",
    "other": "Sonstiges (gesperrt)",
    "networking": "Networking (gesperrt)",
    "back_to_back": "hintereinander",
    "call_morning": "vor dem Beginn",
    "call_evening": "nach dem Ende",
}


# ---------- Kalender ----------------------------------------------------------


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


def period_for(today):
    """(erster Tag, letzter Tag): Montag sechs Wochen vor der aktuellen Woche bis gestern."""
    monday = today - dt.timedelta(days=today.weekday())
    return monday - dt.timedelta(weeks=WEEKS), today - dt.timedelta(days=1)


# ---------- Tagwerk nachgebildet (src/day.rs, src/rules.rs) -----------------------


def union(spans):
    out = []
    for f, t in sorted(spans):
        if out and f <= out[-1][1]:
            out[-1] = (out[-1][0], max(out[-1][1], t))
        else:
            out.append((f, t))
    return out


def fill_gaps(spans):
    out = []
    for f, t in union(spans):
        if out and f - out[-1][1] < MIN_PAUSE:
            out[-1] = (out[-1][0], max(out[-1][1], t))
        else:
            out.append((f, t))
    return out


def clip(spans, lo, hi):
    return [(max(f, lo), min(t, hi)) for f, t in spans if min(t, hi) > max(f, lo)]


class Day:
    def __init__(self, periods, blocks=(), kind=None):
        self.periods = list(periods)
        self.blocks = list(blocks)  # (von, bis, Notiz) – Arbeit ohne Mac
        self.kind = kind  # Regelverstoß oder Art der Arbeit ohne Mac

    def shift(self, by):
        self.periods = [(f + by, t + by) for f, t in self.periods]
        self.blocks = [(f + by, t + by, n) for f, t, n in self.blocks]

    def first(self):
        return min([f for f, _ in self.periods] + [f for f, _, _ in self.blocks])

    def last(self):
        return max([t for _, t in self.periods] + [t for _, t, _ in self.blocks])

    def unframed(self):
        """Arbeitsabschnitte ohne Rahmen: kurze Lücken gefüllt, Arbeit ohne Mac dazu."""
        return fill_gaps(self.periods + [(f, t) for f, t, _ in self.blocks])

    def spans(self):
        return clip(self.unframed(), *FRAME)

    def work(self):
        return sum(t - f for f, t in self.spans())

    def outside(self):
        all_ = self.unframed()
        out = clip(all_, 0, FRAME[0]) + clip(all_, FRAME[1], DAY)
        return [s for s in out if s[1] - s[0] >= 60]


def warnings(days):
    """Hinweise je Tag wie rules::day_warnings."""
    out, prev = {}, None
    for d in sorted(days):
        day, found = days[d], set()
        spans, work = day.spans(), day.work()
        pause = sum(b[0] - a[1] for a, b in zip(spans, spans[1:]) if b[0] - a[1] >= 15 * M)
        if work > 9 * H and pause < 45 * M:
            found.add("Pause 45 Min.")
        elif work > 6 * H and pause < 30 * M:
            found.add("Pause 30 Min.")
        if work > MAX_WORK:
            found.add("über 10 Std.")
        if spans and prev is not None:
            rest = (d - prev[0]).days * DAY + spans[0][0] - prev[1]
            if rest < MIN_REST:
                found.add("Ruhezeit")
        if day.outside():
            found.add("außerhalb Rahmen")
        if found:
            out[d] = found
        if work > 0:
            prev = (d, spans[-1][1])
    return out


# ---------- Tage bauen ----------------------------------------------------------


def counts(lock):
    """Zählt eine Sperre als Arbeitszeit? (kurz oder als Arbeit ohne Mac nachgetragen)"""
    _, dur, note = lock
    return note is not None or dur < MIN_PAUSE


def build_day(start, work, locks):
    """Perioden (entsperrt) und Arbeit ohne Mac für einen Tag.

    `work` ist die gezählte Arbeitszeit der Perioden samt der Sperren, die zählen.
    `locks` sind (Position in gezählter Arbeitszeit, Dauer, Notiz): Notiz None = normale
    Sperre (ab 15 Min. eine Pause), sonst als Arbeit ohne Mac nachgetragen."""
    periods, blocks, t, elapsed = [], [], start, 0
    for lock in sorted(locks, key=lambda l: l[0]):
        pos, dur, note = lock
        run = pos - elapsed
        assert run > 0, (start, work, locks)
        periods.append((t, t + run))
        if note is not None:
            blocks.append((t + run, t + run + dur, note))
        t += run + dur
        elapsed = pos + (dur if counts(lock) else 0)
    assert work > elapsed, (start, work, locks)
    periods.append((t, t + (work - elapsed)))
    return periods, blocks


def free_pos(rng, lo, hi, dur, note, taken, gap=30 * M):
    """Position für eine Sperre mit Abstand zu den anderen (oder None)."""
    if hi < lo:
        return None
    own = dur if counts((0, dur, note)) else 0
    for _ in range(50):
        pos = rng.randint(lo, hi)
        if all(
            pos > p + (d if counts((p, d, n)) else 0) + gap or pos + own + gap < p
            for p, d, n in taken
        ):
            return pos
    return None


def short_locks(rng, work, taken):
    """0–3 kurze Sperren (3–12 Min.) über den Tag verteilt, nicht in der Nähe anderer Sperren."""
    out = []
    for _ in range(rng.randint(0, 3)):
        dur = rng.randint(3 * M, 12 * M) + rng.randint(0, 59)
        pos = free_pos(rng, 45 * M, work - 45 * M, dur, None, taken + out)
        if pos is not None:
            out.append((pos, dur, None))
    return out


def normal_day(rng, work, variant, min_start):
    """Ein Tag mit `work` gezählter Arbeitszeit; `variant` ist die Art der Arbeit ohne Mac,
    NO_BREAK (durchgearbeitet: keine Sperre ab 15 Min. – einziger Regelverstoß des Tages)
    oder None. Gibt None zurück, wenn die Art nicht in den Tag passt."""
    call = rng.randint(15, 40) * M + rng.randint(0, 59) if variant in ("call_morning", "call_evening") else 0
    pwork = work - call  # Arbeitszeit an den Perioden
    start = int(min(max(rng.gauss(8 * H, 35 * M), EARLIEST), 9 * H + 30 * M)) + rng.randint(0, 59)
    locks = []
    if variant == NO_BREAK:
        pass  # keine Mittags- und keine Kaffeepause, nur kurze Sperren
    elif work > 6 * H or rng.random() < 0.5:
        # Mittagspause gegen 12–13 Uhr; ab 9 Std. Arbeit mindestens 45 Min.
        lunch = rng.randint(45, 60) if work > 9 * H else rng.randint(30, 55)
        pos = min(max(12 * H + rng.randint(0, 50) * M - start, 3 * H), pwork - 2 * H)
        locks.append((pos, lunch * M + rng.randint(0, 59), None))
    if variant != NO_BREAK and work > 6 * H and rng.random() < 0.3:
        # Kaffeepause am Nachmittag (zählt als Pause)
        locks.append((int(pwork * 0.8), rng.randint(15, 22) * M + rng.randint(0, 59), None))
    if variant == "call_pause" or variant in LOCKED:
        if variant == "call_pause":
            dur, note = rng.randint(20, 35) * M + rng.randint(0, 59), "Telefonat"
        else:
            (lo, hi), note = LOCKED[variant]
            dur = rng.randint(lo, hi) * M
        pos = free_pos(rng, 75 * M, pwork - dur - 60 * M, dur, note, locks)
        if pos is None:
            return None
        locks.append((pos, dur, note))
    locks += short_locks(rng, pwork, locks)
    day = Day(*build_day(start, pwork, locks), kind=variant)
    if variant == "back_to_back":
        # Meeting und direkt anschließend ein Telefonat (mindestens 15 bzw. 20 Min.)
        (f, t, _), = day.blocks
        cut = f + rng.randint(30 * M, t - f - 20 * M)
        day.blocks = [(f, cut, "Meeting"), (cut, t, "Telefonat")]
    if variant == "call_morning":
        gap = rng.randint(20, 45) * M + rng.randint(0, 59)
        first = day.first()
        day.blocks.insert(0, (first - gap - call, first - gap, "Telefonat"))
    elif variant == "call_evening":
        gap = rng.randint(30, 75) * M + rng.randint(0, 59)
        last = day.last()
        day.blocks.append((last + gap, last + gap + call, rng.choice(["Telefonat", "Telefonat", "Meeting"])))
    # in die erlaubte Zeit schieben: nicht vor 7 Uhr bzw. vor Ende der Ruhezeit,
    # spätestens um 20 Uhr fertig
    lower = max(EARLIEST, min_start)
    if day.first() < lower:
        day.shift(lower - day.first() + rng.randint(0, 15 * M))
    if day.last() > LATEST_END:
        day.shift(-min(day.last() - LATEST_END, day.first() - lower))
    if day.last() > LATEST_END:
        return None
    if variant != "call_pause":
        # Nachgetragen: Uhrzeitfelder übernehmen nur volle Minuten. Die Lücke unter einer
        # Minute zur erfassten Periode zählt wie eine kurze Sperre als Arbeitszeit.
        day.blocks = [(f // M * M, t // M * M, n) for f, t, n in day.blocks]
    return day


def special_day(rng, kind):
    """Tage mit Regelverstoß."""
    s = rng.randint(0, 59)
    if kind == "late_end":  # Ende 21:2x – zusammen mit "early_start" am Folgetag < 11 Std. Ruhe
        p = build_day(11 * H + 40 * M + s, 9 * H, [(4 * H, 45 * M + 13, None), (7 * H, 6 * M + 40, None)])
    elif kind == "early_start":
        p = build_day(7 * H + 4 * M + s, 8 * H, [(5 * H, 40 * M + 7, None)])
    elif kind == "after_22":
        p = build_day(13 * H + 40 * M + s, 8 * H + 30 * M, [(4 * H + 30 * M, 45 * M + 21, None)])
    elif kind == "before_7":
        p = build_day(6 * H + 12 * M + s, 8 * H + 40 * M, [(6 * H, 40 * M + 30, None)])
    elif kind == "pause_20":
        p = build_day(8 * H + 2 * M + s, 7 * H, [(4 * H, 20 * M + 15, None)])
    elif kind == "pause_30_long":
        p = build_day(7 * H + 30 * M + s, 9 * H + 20 * M, [(4 * H + 30 * M, 30 * M + 40, None), (2 * H, 8 * M, None)])
    else:
        raise ValueError(kind)
    return Day(*p, kind=kind)


# ---------- Zeitraum planen -----------------------------------------------------


def place_specials(mondays, workdays):
    """Datum → Art des Regelverstoßes (Wunschtag, sonst ein anderer Tag der Woche)."""
    out = {}
    for week, kinds, candidates in SPECIALS:
        if week >= len(mondays):
            continue
        for wd in candidates:
            dates = [mondays[week] + dt.timedelta(days=wd + i) for i in range(len(kinds))]
            if all(d in workdays and d not in out and d.weekday() < 5 for d in dates):
                out.update(zip(dates, kinds))
                break
    return out


def generate(first, last, seed):
    rng = random.Random(seed)
    hol = set().union(*(holidays(y) for y in range(first.year, last.year + 1)))
    all_days = [first + dt.timedelta(days=i) for i in range((last - first).days + 1)]
    workdays = [d for d in all_days if d.weekday() < 5 and d not in hol]
    mondays = sorted({d - dt.timedelta(days=d.weekday()) for d in all_days})
    specials = place_specials(mondays, set(workdays))

    # Arbeit ohne Mac: rund 25 % der Werktage, jede Art etwa gleich oft
    free = [d for d in workdays if d not in specials]
    count = min(len(free), round(len(workdays) * WORK_SHARE))
    variants = VARIANTS[:]
    rng.shuffle(variants)
    # gleichmäßig über den Zeitraum verteilt: je ein Tag aus jedem Abschnitt
    picks = [rng.choice(free[i * len(free) // count:(i + 1) * len(free) // count]) for i in range(count)]
    away = dict(zip(picks, (variants * count)[:count]))

    # Mehr als 6 Std. ohne Pause: rund 10 % der Werktage, ebenfalls gleichmäßig verteilt
    free = [d for d in free if d not in away]
    count = min(len(free), round(len(workdays) * NO_BREAK_SHARE))
    no_break = {rng.choice(free[i * len(free) // count:(i + 1) * len(free) // count]) for i in range(count)}

    # Arbeitszeit je Tag: Wochensoll anteilig (bei Feiertagen weniger), ±8 % Zufall
    plan, fixed = {}, {}
    for d, kind in specials.items():
        fixed[d] = special_day(rng, kind)
    for monday in mondays:
        week = [d for d in workdays if 0 <= (d - monday).days < 7]
        open_days = [d for d in week if d not in fixed]
        if not open_days:
            continue
        target = WEEK_TARGET * len(week) / 5 * rng.uniform(0.92, 1.08)
        rest = target - sum(fixed[d].work() for d in week if d in fixed)
        bounds = [NO_BREAK_RANGE if d in no_break else WORK_RANGE for d in open_days]
        work = [min(max(rest / len(open_days) + rng.uniform(-1.9, 1.9) * H, lo), hi) for lo, hi in bounds]
        # Summe angleichen, ohne die Grenzen zu verlassen
        for _ in range(50):
            diff = rest - sum(work)
            if abs(diff) < M:
                break
            work = [min(max(w + diff / len(work), lo), hi) for w, (lo, hi) in zip(work, bounds)]
        for d, w in zip(open_days, work):
            plan[d] = int(w) // 60 * 60 + rng.randint(0, 59)

    # Tage der Reihe nach bauen – so kennt jeder Tag das Ende des Vortags (Ruhezeit)
    days, prev = {}, None
    for d in workdays:
        if d in fixed:
            day = fixed[d]
        else:
            min_start = 0
            if prev is not None:
                min_start = prev[1] + MIN_REST - (d - prev[0]).days * DAY + rng.randint(5, 25) * M
            variant = NO_BREAK if d in no_break else away.get(d)
            day = normal_day(rng, plan[d], variant, min_start)
            for fallback in () if variant == NO_BREAK else ("meeting", "call_pause", None):
                if day is not None:
                    break
                day = normal_day(rng, plan[d], fallback, min_start)
            if day is None:
                raise RuntimeError(f"{d}: kein passender Tagesablauf")
        days[d] = day
        prev = (d, min(day.last(), FRAME[1]))
    return days, hol


def expected_warnings(days):
    out = {}
    for d, day in days.items():
        if day.kind in EXPECTED:
            out[d] = EXPECTED[day.kind]
    return out


def no_break_days(days):
    return [d for d in sorted(days) if days[d].kind == NO_BREAK]


def no_break_ok(days):
    """Tage ohne Pause: ein einziger Arbeitsabschnitt von mehr als 6 Std."""
    return all(len(days[d].spans()) == 1 and days[d].work() > 6 * H for d in no_break_days(days))


# ---------- Ausgabe -------------------------------------------------------------


def clock(s):
    return f"{s // 3600:02d}:{s % 3600 // 60:02d}:{s % 60:02d}"


def date_cell(d):
    return f"{WEEKDAYS[d.weekday()]}, {d:%d.%m.%Y}"


def write_markdown(out, days, first, last, seed):
    out.write(f"# Tagwerk – Testdaten {first:%d.%m.%Y} bis {last:%d.%m.%Y}\n\n")
    out.write(f"Erzeugt mit testdata/generate_testdata.py (Startwert {seed}).\n\n")
    out.write("## Korrekturen\n\n")
    out.write(f"Art: {KIND_WORK} (z. B. Telefonat oder Meeting).\n\n")
    out.write("| Datum | Art | Von | Bis | Notiz |\n|:------|:----|:---:|:---:|:------|\n")
    for d in sorted(days):
        for f, t, note in days[d].blocks:
            out.write(f"| {date_cell(d)} | {KIND_WORK} | {clock(f)} | {clock(t)} | {note} |\n")
    out.write("\n## Erfasste Perioden\n\n| Datum | Von | Bis |\n|:------|:---:|:---:|\n")
    for d in sorted(days):
        for f, t in days[d].periods:
            out.write(f"| {date_cell(d)} | {clock(f)} | {clock(t)} |\n")


def report(days, hol, first, last, seed, found):
    """Kontrolle auf stderr: Wochensummen, Arbeit ohne Mac, Regelverstöße."""
    err = sys.stderr
    print(f"Zeitraum {first:%d.%m.%Y} bis {last:%d.%m.%Y}, Startwert {seed}", file=err)
    weeks = {}
    for d, day in days.items():
        weeks.setdefault(d.isocalendar()[:2], []).append(day.work())
    for (year, kw), ws in sorted(weeks.items()):
        total = sum(ws)
        print(
            f"  KW {kw:2d}/{year}: {len(ws)} Tage, {total / H:5.2f} Std. "
            f"({(total / (WEEK_TARGET * len(ws) / 5) - 1) * 100:+.1f} %), "
            f"Tage {min(ws) / H:.2f}–{max(ws) / H:.2f} Std.",
            file=err,
        )
    away = [d for d in sorted(days) if days[d].blocks]
    print(f"Arbeit ohne Mac an {len(away)} von {len(days)} Werktagen:", file=err)
    for d in away:
        blocks = ", ".join(f"{clock(f)[:5]}–{clock(t)[:5]} {n or 'Sonstiges'}" for f, t, n in days[d].blocks)
        print(f"  {date_cell(d)}  {VARIANT_NAMES[days[d].kind]:22s} {blocks}", file=err)
    through = no_break_days(days)
    print(
        f"Mehr als 6 Std. ohne Pause an {len(through)} von {len(days)} Werktagen "
        f"({len(through) / len(days) * 100:.0f} %):",
        file=err,
    )
    for d in through:
        (f, t), = days[d].spans()
        print(f"  {date_cell(d)}  {clock(f)[:5]}–{clock(t)[:5]}  {(t - f) / H:.2f} Std.", file=err)
    print("Regelverstöße:", file=err)
    for d in sorted(found):
        print(f"  {date_cell(d)}  {', '.join(sorted(found[d]))}", file=err)
    in_range = sorted(h for h in hol if first <= h <= last)
    print(f"Feiertage im Zeitraum (bundesweit): {', '.join(f'{h:%d.%m.%Y}' for h in in_range) or 'keine'}", file=err)


def main():
    ap = argparse.ArgumentParser(description="Testdaten für Tagwerk erzeugen (Markdown auf stdout).")
    ap.add_argument("--today", type=dt.date.fromisoformat, default=dt.date.today(),
                    help="Stichtag JJJJ-MM-TT (Standard: heute); die Daten enden am Vortag")
    ap.add_argument("--seed", type=int, default=SEED, help=f"Startwert des Zufallsgenerators (Standard: {SEED})")
    args = ap.parse_args()
    first, last = period_for(args.today)

    # Sollte ein Tag ungewollt gegen eine Regel verstoßen, mit dem nächsten Startwert neu würfeln.
    for seed in range(args.seed, args.seed + 100):
        days, hol = generate(first, last, seed)
        found = warnings(days)
        if found == expected_warnings(days) and no_break_ok(days):
            break
    else:
        sys.exit("Keine Testdaten ohne ungewollte Regelverstöße gefunden.")
    write_markdown(sys.stdout, days, first, last, seed)
    report(days, hol, first, last, seed, found)


if __name__ == "__main__":
    main()
