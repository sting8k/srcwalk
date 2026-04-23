#!/usr/bin/env python3
"""
Compare old srcwalk (built-in tools) vs new srcwalk (MCP-only) exploration patterns.
"""

import json
from pathlib import Path
from typing import Dict, List

def parse_jsonl(file_path: str) -> List[Dict]:
    """Parse JSONL file and return list of run results."""
    results = []
    with open(file_path, 'r') as f:
        for line in f:
            line = line.strip()
            if line:
                results.append(json.loads(line))
    return results

def main():
    results_dir = Path("/Users/flysikring/conductor/workspaces/srcwalk/almaty/benchmark/results")

    old_file = results_dir / "benchmark_20260213_131246.jsonl"
    new_file = results_dir / "benchmark_20260213_135039.jsonl"

    old_runs = parse_jsonl(str(old_file))
    new_runs = parse_jsonl(str(new_file))

    # Filter to valid runs only
    old_runs = [r for r in old_runs if 'error' not in r]
    new_runs = [r for r in new_runs if 'error' not in r]

    print("="*80)
    print("OLD SRCWALK (with built-in tools) vs NEW SRCWALK (MCP-only)")
    print("="*80)

    print(f"\nOld file: {old_file.name}")
    print(f"New file: {new_file.name}")

    # Group by task and mode
    def group_by_task_mode(runs):
        groups = {}
        for r in runs:
            key = (r['task'], r['mode'])
            if key not in groups:
                groups[key] = []
            groups[key].append(r)
        return groups

    old_groups = group_by_task_mode(old_runs)
    new_groups = group_by_task_mode(new_runs)

    # Compare srcwalk runs only
    print("\n" + "="*80)
    print("SRCWALK MODE COMPARISON (Old built-in vs New MCP-only)")
    print("="*80)

    all_tasks = sorted(set(k[0] for k in old_groups.keys() if k[1] == 'srcwalk'))

    for task in all_tasks:
        old_srcwalk = old_groups.get((task, 'srcwalk'), [])
        new_srcwalk = new_groups.get((task, 'srcwalk'), [])

        if old_srcwalk and new_srcwalk:
            print(f"\n{'='*80}")
            print(f"Task: {task}")
            print(f"{'='*80}")

            for old, new in zip(old_srcwalk, new_srcwalk):
                print(f"\nOLD (built-in): {old.get('srcwalk_version', 'unknown')}")
                print(f"  Turns: {old['num_turns']}, Tool calls: {old['num_tool_calls']}")
                print(f"  Tools: {old['tool_calls']}")
                print(f"  Correct: {old['correct']}")

                print(f"\nNEW (MCP-only): {new.get('srcwalk_version', 'unknown')}")
                print(f"  Turns: {new['num_turns']}, Tool calls: {new['num_tool_calls']}")
                print(f"  Tools: {new['tool_calls']}")
                print(f"  Correct: {new['correct']}")

                # Calculate differences
                turn_delta = new['num_turns'] - old['num_turns']
                tool_delta = new['num_tool_calls'] - old['num_tool_calls']

                print(f"\nDELTA:")
                print(f"  Turns: {turn_delta:+d} ({'more' if turn_delta > 0 else 'fewer' if turn_delta < 0 else 'same'})")
                print(f"  Tool calls: {tool_delta:+d} ({'more' if tool_delta > 0 else 'fewer' if tool_delta < 0 else 'same'})")
                print(f"  Correctness: {'same' if old['correct'] == new['correct'] else 'CHANGED'}")

    # Summary statistics
    print("\n" + "="*80)
    print("SUMMARY STATISTICS")
    print("="*80)

    old_srcwalk_runs = [r for r in old_runs if r['mode'] == 'srcwalk' and r['model'] == 'sonnet']
    new_srcwalk_runs = [r for r in new_runs if r['mode'] == 'srcwalk' and r['model'] == 'sonnet']

    def avg(runs, key):
        values = [r[key] for r in runs if key in r]
        return sum(values) / len(values) if values else 0

    print(f"\n{'Metric':<30} {'Old (built-in)':>20} {'New (MCP-only)':>20} {'Delta':>15}")
    print("-" * 90)

    metrics = [
        ('num_turns', 'Avg turns'),
        ('num_tool_calls', 'Avg tool calls'),
    ]

    for key, label in metrics:
        old_avg = avg(old_srcwalk_runs, key)
        new_avg = avg(new_srcwalk_runs, key)
        delta = new_avg - old_avg
        print(f"{label:<30} {old_avg:>20.2f} {new_avg:>20.2f} {delta:>15.2f}")

    # Correctness comparison
    old_correct = sum(1 for r in old_srcwalk_runs if r.get('correct'))
    new_correct = sum(1 for r in new_srcwalk_runs if r.get('correct'))

    print(f"\n{'Correctness':<30} {old_correct}/{len(old_srcwalk_runs):>18} {new_correct}/{len(new_srcwalk_runs):>18} {new_correct - old_correct:>15}")

    # Tool mix comparison
    print("\n" + "="*80)
    print("TOOL MIX ANALYSIS")
    print("="*80)

    def count_tools(runs):
        tool_counts = {}
        for r in runs:
            for tool, count in r.get('tool_calls', {}).items():
                tool_counts[tool] = tool_counts.get(tool, 0) + count
        return tool_counts

    old_tools = count_tools(old_srcwalk_runs)
    new_tools = count_tools(new_srcwalk_runs)

    all_tools = sorted(set(list(old_tools.keys()) + list(new_tools.keys())))

    print(f"\n{'Tool':<40} {'Old':>15} {'New':>15} {'Delta':>15}")
    print("-" * 90)

    for tool in all_tools:
        old_count = old_tools.get(tool, 0)
        new_count = new_tools.get(tool, 0)
        delta = new_count - old_count
        print(f"{tool:<40} {old_count:>15} {new_count:>15} {delta:>15}")

if __name__ == "__main__":
    main()
