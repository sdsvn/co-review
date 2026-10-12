## ADDED Requirements

### Requirement: Phone view
Co-Review SHALL serve a phone view at `/m/` with its API at `/api/m`, to read, reply and submit, for localhost and
the hosts in `CO_REVIEW_ALLOWED_HOSTS`.

#### Scenario: Over Tailscale
- **WHEN** the reviewer opens `/m/` from a phone on an allowed host
- **THEN** they can read threads, reply and submit

### Requirement: Review server
`co-review-server [-dir <dir> | <dir>] [-md] [-patch] [-addr] [-store] [-openspec]` SHALL open the review in
Co-Review, print its address, serve `/api/context`, `POST /api/threads/<id>/messages` and `POST /api/findings`,
and write the state to `-store`.

#### Scenario: Script adds a finding
- **WHEN** a script posts to `/api/findings`
- **THEN** the finding appears in the review as proposed
