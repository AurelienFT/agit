# Business model

Agit follows the **GitLab open-core model**, sharpened by the self-hosted runner: the runtime and the core developer-facing product are free and self-hostable; team/org features that only matter at scale (compliance, governance across many repos, support) are paid.

> **Free people can run real Agit. Companies pay for the controls and the operations they need to deploy it at scale.**

This document captures the intent of the model and the rough split between tiers. It is **not a price list** and will evolve with user research.

## The self-hosted runner reshapes the tier split

The two-component split is the single most important business-model decision:

```
agit-server     orchestrates  ──▶ can be Cloud (paid) OR self-hosted (free)
agit-runner     does the work ──▶ ALWAYS self-hosted, ALWAYS open source
```

Even on the paid Cloud tier, the runner stays in the customer's infra. Their code and credentials never transit through Agit Cloud. That property is non-negotiable — it's the whole point of the pivot.

This gives three concrete deployment shapes, all built from the same OSS code:

| Shape | Server | Runner | Who runs it | Price |
|---|---|---|---|---|
| **Pure CE** | self-hosted | self-hosted | Customer hosts both | Free |
| **Cloud hybrid** | Agit-managed | customer-hosted | Customer + Agit | Paid (Cloud tier) |
| **Enterprise** | self-hosted *or* Agit-managed | customer-hosted | Same as Cloud, plus governance | Paid (Enterprise) |

In all three shapes, the runner is the same OSS binary.

## Tiers

### Community Edition (free, open source, self-hosted)

Open source. License: TBD — likely MIT or Apache-2.0 for the runtime, possibly a CLA for enterprise components.

Includes everything needed to use Agit on a single team / single org:

- **The entire runner** (`agit-runner` crate) — every provider implementation, every policy guardrail, every Git output.
- **The whole `agit-core` library** — schema, policy engine, run state model.
- **The full server** (`agit-server` crate) — webhooks, missions queue, runner-facing API, dashboard.
- Agent declaration in `.agit/agents.yaml`: providers + agents.
- All declared trigger types: `github_issue_label`, `github_pull_request`, `github_comment_command`, `manual`.
- All declared output kinds: `pull_request`, `blocking_review`, `comment`, `patch`.
- All declared provider kinds: `local_command`, `anthropic_api`, `openai_api`, `openai_compatible`.
- SQLite-backed deployment (single-node).
- `docker compose` template for spinning up server + runner together.

You can deploy CE on your own infra, point it at your repos, and never pay Agit. This is intentional and load-bearing for adoption.

### Agit Cloud (paid SaaS)

Agit hosts `agit-server`. The customer installs the runner in their own infra; **the runner is the same OSS binary as CE**.

Cloud sells:

- **Hosted dashboard** with the operational features customers don't want to maintain themselves (backups, upgrades, status page, SLO).
- **Org-level views** across many repos and many runners.
- **Webhooks reliability** — retries, dead-letter queues, audit of received vs processed events.
- **Cost analytics** aggregated across runners and providers.
- **Priority support** and account management.

Billing axis TBD — probably per-seat with a soft usage cap, to mirror GitHub's pricing surface.

### Enterprise / self-managed (paid)

Either delivered on top of Cloud, or as a license-keyed self-managed deployment of the same image. Adds the controls compliance and security buyers ask for:

#### 1. Org-wide governance

- **Cross-repo policy** — define org-level rules ("no agent may write `**/auth/**` without `security_reviewer` approval") that override per-repo configs.
- **Audit log** — immutable trail of every Run, policy decision, PR opened, runner registration, human override. Exportable for SOC 2 / ISO 27001.
- **Approval workflows** — required human sign-off on specific path patterns, branches (`main`), or agent classes.
- **Compliance reports** — agentic PR volume, merge/reject ratio, violations by agent, runner inventory.

#### 2. Identity & access

- SSO (SAML, OIDC).
- SCIM provisioning.
- Granular RBAC (admin / policy author / reviewer / runner operator / read-only).
- Per-team isolation of Runs and dashboards.

#### 3. Scale & operations

- Postgres-backed deployment with HA.
- Distributed runners — many runners against one server, with per-team or per-repo affinities.
- Custom retention for Run logs and artifacts.
- Backup/restore tooling.
- Priority support and SLA.

#### 4. Insight

- Cost analytics across repos, agents, and providers.
- Quality analytics: which agents introduce regressions, which get rejected, which save reviewer time.
- Cross-repo agent marketplace and shareable agent templates.
- Advanced triggers (cron, multi-event, conditional pipelines).
- Run replay / debugger.

## What is *not* paywalled

To keep the open-core honest:

- **The runner is never paywalled.** Anything that touches customer code or secrets stays OSS forever. That's the trust posture.
- **The schema** of `.agit/agents.yaml` is not split between tiers. The same YAML runs on CE and Enterprise.
- **All provider kinds** are CE: `local_command`, `anthropic_api`, `openai_api`, `openai_compatible`.
- **Per-repo policy** is CE. Only **org-wide policy** spanning repos is paid.
- **The dashboard** is CE. Only org-level analytics and audit views are paid.
- **No usage caps** in CE on issues/PRs/runs. Hosting CE is the only cost CE has.

## Why this works for Agit specifically

The wedge — *self-hostable control plane for AI-generated software contributions* — naturally splits into two halves:

| Half                                             | Who needs it           | Tier        |
|--------------------------------------------------|------------------------|-------------|
| "Let one team safely use agents on their repos, in their infra." | Every dev team         | Community   |
| "Prove to compliance that the whole org's agent use is controlled, audited, and bounded — at scale." | Security/legal/exec    | Enterprise  |

The first half is a developer tool; OSS distribution is the right channel. The second half is a control plane for execs and compliance; SaaS or enterprise license is the right channel. Same product, two buyers.

The self-hosted runner makes the buyer story easier on both sides:

- For the dev team: *nothing leaves my infra; I can adopt this without a security review*.
- For the buyer: *I get a polished orchestration UX, my devs get a frictionless tool, and our code never crossed the vendor boundary*.

## Open questions

- **Pricing axis** — per-seat, per-repo, per-Run, per-runner, or hybrid. Probably per-seat with usage soft-caps, but TBD.
- **License of the runtime** — MIT/Apache for everything, or a Sentry-style BSL/FSL after a delay to deter cloud resellers? Default to permissive until a real risk shows up.
- **Cloud-hosted runner option** — a managed runner image for customers who *don't* want to operate one. Risky for the trust story; should never be the default.
- **Marketplace economics** — free authors, revenue share for premium agent templates, or pure community for v1? Defer.
- **Open governance** — accept external contributions to CE from day one (yes for the runner; probably yes after stabilization for the server).

## Inspirations

- **GitLab** — the canonical open-core dev tool. CE/EE split based on team-vs-org concerns, not on capability gating.
- **Sentry** — runtime is OSS (BSL/FSL after a delay), commercial value comes from hosted SaaS and enterprise features.
- **HashiCorp Terraform / Vault** — OSS core, paid Terraform/Vault Cloud and Enterprise for governance, drift, run insights. Closest analog functionally.
- **Tailscale** — control plane managed by the vendor; data plane (the node) is OSS and self-hosted. Exact same trust posture Agit aims for.

The Tailscale analog is increasingly the cleanest mental model for the runtime side: **server = control plane (managed or self-hosted), runner = data plane (always self-hosted, always OSS)**.
