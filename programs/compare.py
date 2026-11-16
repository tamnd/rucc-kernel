"""Compare the programs results of the rucc kernel with those of the gcc kernel.

    python3 compare.py REFERENCE_DIR OTHER_DIR

Each directory is the OUT of run.sh, and the OUT of each boot that ran only the rates step is
a directory rates-N inside it, whose results count with the others. The other kernel fails the
comparison when a step that passed on the reference did not pass on it, when a set key has a
value the reference does not have (a failed unit, test or stressor), when its dmesg has a kind
of bad line the reference's does not, or when a rate is slower the way rk bench means it: the
median is more than 5% under the reference's, the one-ratio limit of rk bench, and the 95%
bootstrap interval of the ratio is under 1 too. The report goes to standard output as Markdown,
and the exit status is 1 when the comparison fails.
"""

import random
import re
import statistics
import sys
from collections import defaultdict
from pathlib import Path

# Keys whose values are compared as sets.
SETS = {
    "failed-unit",
    "git-failed",
    "python-failed",
    "stress-failed",
    "fs-failed",
    "podman-failed",
    "failed",
}

# Keys whose values are rates, higher is better.
RATES = {"nginx-rps", "nginx-big-rps", "pgbench-tps"}

# The most a rate can be under the reference's, as a ratio.
LIMIT = 1.05

# How many times the bootstrap draws the samples again.
DRAWS = 2000

# The dmesg lines that are trouble, whatever the kernel.
BAD = re.compile(
    r"BUG:|Oops|WARNING:|general protection|soft lockup|hard LOCKUP|blocked for more than"
    r"|rcu: INFO|self-detected stall|UBSAN|KASAN|corruption|refcount_t|Kernel panic"
)


def runs(directory):
    """The OUT of the main boot and of each rates boot."""
    return [Path(directory)] + sorted(Path(directory).glob("rates-*"))


def load(directory):
    """The results of every boot, as {step: {key: [values]}}."""
    results = defaultdict(lambda: defaultdict(list))
    for run in runs(directory):
        path = run / "results.tsv"
        if path.exists():
            for line in path.read_text().splitlines():
                parts = line.split("\t", 2)
                if len(parts) == 3:
                    results[parts[0]][parts[1]].append(parts[2])
    return results


def bad_lines(directory):
    """The kinds of bad dmesg lines, with times, numbers and addresses taken out."""
    kinds = set()
    for run in runs(directory):
        path = run / "dmesg.txt"
        if not path.exists():
            continue
        for line in path.read_text(errors="replace").splitlines():
            line = re.sub(r"^\[[ 0-9.]*\] ", "", line)
            if BAD.search(line):
                kinds.add(re.sub(r"0x[0-9a-f]+|\b[0-9]+\b", "N", line).strip())
    return kinds


def numbers(values):
    found = []
    for value in values:
        try:
            found.append(float(value))
        except ValueError:
            pass
    return found


def interval(reference, other):
    """The 95% bootstrap interval of the ratio of the medians, other over reference."""
    draw = random.Random(0)
    ratios = []
    for _ in range(DRAWS):
        x = statistics.median(draw.choices(reference, k=len(reference)))
        y = statistics.median(draw.choices(other, k=len(other)))
        if x:
            ratios.append(y / x)
    ratios.sort()
    if not ratios:
        return None
    return ratios[int(len(ratios) * 0.025)], ratios[int(len(ratios) * 0.975) - 1]


def main():
    reference, other = sys.argv[1], sys.argv[2]
    ours, theirs = load(other), load(reference)
    problems = []
    rows = []
    for step in list(dict.fromkeys(list(theirs) + list(ours))):
        a, b = theirs.get(step, {}), ours.get(step, {})
        for key in list(dict.fromkeys(list(a) + list(b))):
            left, right = a.get(key, []), b.get(key, [])
            if key == "status":
                passed = bool(left) and all(v == "0" for v in left)
                fine = not passed or (bool(right) and all(v == "0" for v in right))
                rows.append((step, key, " ".join(left), " ".join(right), fine))
                if not fine:
                    problems.append(f"{step} passed on the reference and ended {right} here")
            elif key in SETS:
                new = sorted(set(right) - set(left))
                rows.append((step, key, len(set(left)), len(set(right)), not new))
                if new:
                    problems.append(f"{step} {key} only here: {', '.join(new)}")
            elif key in RATES:
                xs, ys = numbers(left), numbers(right)
                x = statistics.median(xs) if xs else None
                y = statistics.median(ys) if ys else None
                if not x:
                    fine, note = True, ""
                elif not y:
                    fine, note = False, ""
                else:
                    low, high = interval(xs, ys)
                    fine = y * LIMIT >= x or high >= 1
                    note = f" ({y / x:.3f}, {low:.3f} to {high:.3f})"
                rows.append((step, key, f"{x} n={len(xs)}", f"{y} n={len(ys)}{note}", fine))
                if not fine:
                    problems.append(f"{step} {key} median {y} against {x}")
            else:
                rows.append((step, key, " ".join(left), " ".join(right), True))
    dmesg = sorted(bad_lines(other) - bad_lines(reference))
    if dmesg:
        problems.append("dmesg lines only here: " + "; ".join(dmesg[:20]))
    print("| step | key | gcc | rucc | |\n|---|---|---|---|---|")
    for step, key, x, y, fine in rows:
        print(f"| {step} | {key} | {x} | {y} | {'' if fine else 'worse'} |")
    print()
    # Every kind of bad line on either kernel, and not only the ones the rucc kernel adds, so that a
    # run that should have none says so whichever kernel the line came from.
    for name, directory in (("gcc", reference), ("rucc", other)):
        kinds = sorted(bad_lines(directory))
        print(f"Kinds of bad dmesg line on the {name} kernel: {len(kinds)}")
        for kind in kinds[:20]:
            print(f"    {kind}")
    print()
    if problems:
        print("The rucc kernel does worse than the gcc kernel:\n")
        for problem in problems:
            print(f"- {problem}")
        return 1
    print("The rucc kernel does as well as the gcc kernel on every step.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
