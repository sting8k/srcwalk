# Source-navigation benchmark — 2026-07-27

## Summary

This benchmark compares source-navigation agents without srcwalk against the latest srcwalk build recorded in the July 27 benchmark sessions.

The latest recorded build reduced estimated context pressure in three of five comparisons while producing rubric scores close to the baselines. The effect was task-dependent, so the evidence does not support a universal efficiency or answer-quality claim.

## Test setup

The benchmark compares two agent configurations:

- **Baseline:** source navigation without srcwalk.
- **srcwalk:** the latest build recorded in the July 27 benchmark sessions.

Every comparison uses GPT-5.5 with the same thinking level for both configurations. Answers must cite exact `path:line` evidence, distinguish observed source behavior from inference, and remain under 1,200 words.

The reported dimensions are:

- task-specific semantic rubric score,
- and estimated context pressure.

## Tasks

### Effect HTTP streaming — GPT-5.5 medium

A long-lived HTTP handler streams `text/event-stream` content under Node. Writes encounter backpressure and the client disconnects before completion.

The agent must trace the response from the handler to the Node wire, explain when headers and bytes become committed, and identify ownership of backpressure, disconnect handling, errors, cleanup, and finalization.

### Pebble write durability — GPT-5.5 medium

A low-latency sync write returns and the new value is immediately readable in-process. Waiting for durability later reports an fsync error, after which the process crashes and reopens the database.

The agent must determine whether the value is guaranteed to survive, tracing the API sequence, durability reporting, WAL replay, and the boundary between Pebble guarantees and unknown storage-device behavior.

### Prometheus configuration reload — GPT-5.5 medium

An operator changes scrape settings and a rule file, then calls `POST /-/reload`. Reload returns HTTP 500 because rule updating fails, but some targets behave as if the new scrape configuration was accepted.

The agent must determine whether this mixed state is possible, which consumers can observe old or new configuration, whether processing stops at the first failure, what gets rolled back, and what status or metrics can truthfully report.

### Ripgrep search dispatch — GPT-5.5 medium

The agent must trace how ripgrep selects single-line or multi-line search, starting from `Searcher` and ending at the matching engine.

The answer must identify the participating structs and files and explain how generic type parameters flow through the dispatch path.

### Kubernetes graceful Pod deletion — GPT-5.5 high

A running Pod is deleted through an API request with `GracePeriodSeconds`.

The agent must trace API storage state, watch propagation, kubelet pod-worker handling, `preStop`, TERM/KILL delivery, and final object removal. It must also explain grace-period shortening, force deletion, ownership of each transition, and remaining races.

## Results

Each row compares srcwalk with a baseline using the same task, model, and thinking level. Percentage columns show the srcwalk delta; negative means less. Pebble reports medians from the two most recent srcwalk 1.3.0 runs and two fresh baseline runs; the other rows are single-run comparisons.

| Task | Rubric score | Context |
| --- | ---: | ---: |
| Effect | 38→38/40 | +6.3% |
| Pebble (median) | 34.5→33.5/40 | -7.7% |
| Prometheus | 35→34/40 | -40.8% |
| Ripgrep | 29→30/30 | -35.0% |
| Kubernetes deletion | 37→36/40 | +2.8% |

## What the results show

- Estimated context pressure fell in three of five comparisons and stayed within baseline range in the other two.
- Effect tied its baseline rubric score; Ripgrep improved by one point.
- Prometheus and Kubernetes deletion lost one rubric point each.
- Pebble's two-run median was one point below baseline while preserving the core conclusion.

srcwalk reduces context pressure substantially in many tasks or stays close to baseline, addressing the bloated-context issue seen in the older 1.2.x builds.
