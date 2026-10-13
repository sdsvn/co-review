# Feature: Rate limiter

## Context

Requests from one client can starve the others. We add a token bucket per client.

## Design

- Limit requests per client — so one client cannot starve the rest
  - Keep a token bucket per client key
  - Refill the bucket every second ⚠ clock skew between nodes
- Reject over-limit requests with 429

The bucket size is configurable. The bucket size defaults to 100 tokens.

Résumé: naïve clients retry immediately — that is the problem.
