"""Summarize zen bench run files (tools/bench.sh output): per model, passed per run with
mean/min/max, mean turns, tool-error rate (tool results with is_error / tool calls, main agent
only), and failures caused by infrastructure (429, lost tunnel, gateway) counted apart.
Usage: python3 tools/bench_summary.py run1.json [run2.json ...]  (also: BENCH_SUMMARY=1 tools/bench.sh ...)
"""

import json
import statistics as st
import sys
from collections import defaultdict


def main(paths):
    by = defaultdict(list)
    for p in paths:
        with open(p) as f:
            d = json.load(f)
        by[d["model"]].append(d)
    for model, runs in by.items():
        passed = [d["passed"] for d in runs]
        rows = [t for d in runs for t in d["tasks"]]
        turns = [t["num_turns"] for t in rows if isinstance(t.get("num_turns"), int)]
        calls = sum(t.get("tool_calls") or 0 for t in rows)
        errs = sum(t.get("tool_errors") or 0 for t in rows)
        infra = sum(1 for t in rows if t.get("infra") and not t["pass"])
        per = ", ".join(
            f"{d['passed']}/{d['total']}"
            f"{' (partial)' if any(t.get('infra') and not t['pass'] for t in d['tasks']) else ''}"
            for d in runs
        )
        print(
            f"{model}: runs={len(runs)} passed=[{per}] mean={st.mean(passed):.1f} "
            f"min={min(passed)} max={max(passed)} "
            f"turns_mean={(st.mean(turns) if turns else 0.0):.1f} tool_calls={calls} "
            f"tool_errors={errs} "
            f"tool_error_rate={(100.0 * errs / calls if calls else 0.0):.1f}% "
            f"infra_failed_tasks={infra}"
        )
        fails = defaultdict(int)
        for t in rows:
            if not t["pass"]:
                fails[t["task"] + ("(infra)" if t.get("infra") else "")] += 1
        if fails:
            print(
                "  failed: "
                + ", ".join(f"{kv[0]} x{kv[1]}" for kv in sorted(fails.items()))
            )


if __name__ == "__main__":
    main(sys.argv[1:])
