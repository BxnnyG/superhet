# Architecture decision records

Format: one file per decision, `NNNN-short-title.md`, sections *Status*, *Context*, *Options*, *Decision*, *Consequences*. Status is one of `proposed`, `accepted`, `superseded by NNNN`, `deprecated`. Accepted ADRs are not edited in substance; write a new ADR that supersedes them.

| ADR | Title | Status |
|---|---|---|
| [0001](0001-backend-dsp-approach.md) | Backend DSP approach | accepted |
| [0002](0002-frontend-framework.md) | Frontend framework | accepted |
| [0003](0003-plugin-execution-model.md) | Plugin execution model | accepted |
| [0004](0004-tetra-demodulator.md) | TETRA receive stack (TMO + DMO) | accepted |
| [0005](0005-codec-plugin-concept.md) | Codec plugin concept | accepted |
| [0006](0006-database.md) | Database | accepted |
| [0007](0007-service-decomposition-and-bus.md) | Service decomposition and message bus | accepted |
| [0008](0008-local-auth.md) | Local admin authentication | accepted |
| [0009](0009-zoom-fft.md) | Zoom FFT (real resolution when zoomed) | accepted |
| [0010](0010-wfm-stereo-and-rds.md) | WFM stereo, stereo audio streams and native RDS | accepted |
| [0011](0011-users-roles-and-guest-access.md) | Users, roles and guest access (amends 0008) | accepted |
| [0012](0012-audit-log-and-admin-console.md) | Audit log and admin console | accepted |
| [0013](0013-dab-via-plugin.md) | DAB/DAB+ via plugin | accepted |
| [0014](0014-plugin-contract-v1-and-host-supervision.md) | Plugin contract v1 and host supervision | accepted |
| [0015](0015-plugin-wiring.md) | Plugin wiring: input taps, claims, persistence, durable events | accepted |
| [0016](0016-dab-plugin-design.md) | DAB plugin design (addendum to 0013) | accepted |
| [0017](0017-adsb-plugin-and-map.md) | ADS-B plugin (readsb) and the aircraft map | accepted |
| [0018](0018-digital-voice-plan.md) | Digital voice plan: M17 bundled, everything encumbered stays external | accepted |
| [0019](0019-oidc-login.md) | Optional OpenID Connect login | accepted |
| [0020](0020-multi-device.md) | Several local SDRs, one daemon each | accepted |
| [0021](0021-tetra-receive-plugin.md) | TETRA receive plugin: own demodulator, osmo-tetra's tetra-rx, the user's ETSI codec | accepted |
| [0022](0022-plugin-ui-panels.md) | Plugin web panels in a sandboxed frame | accepted |
| [0023](0023-device-access.md) | Per-device access policies | accepted |
| [0024](0024-oidc-logout.md) | OIDC logout: RP-initiated and back-channel | accepted |
| [0025](0025-scanner.md) | Scanner in the device daemon, detection on the spectrum | accepted |
| [0026](0026-adsb-aircraft-database-and-range.md) | ADS-B aircraft database (Mictronics, ODC-By) and range statistics | accepted |
| [0027](0027-notifications.md) | Notifications: rules, targets, outbox delivery | accepted |
| [0029](0029-share-links.md) | Public receiver page: share links and the embeddable player | accepted |
