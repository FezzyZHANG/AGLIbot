---
id: n-agli-q-growth-deidentification
graph_id: default
version: 1
title: Reconcile de-identification, personal growth, and outward initiative
kind: question
state: open
in_graph: true
summary: 'A01/A02 先核对脱敏契约：保留抽象经验和人格成长，区分可识别私密细节、纯风格变化与向外披露。明确派生产物的 scope、处理策略、来源和审查门槛；具体修复方案尚未接受。


  Review A01/A02 against the de-identification contract before selecting a fix. Preserve
  useful learning and growth while distinguishing identifying content, abstract style
  changes and outward disclosure. Clarify scope, processing policy, provenance and
  review thresholds; no implementation approach has been accepted.'
created_at: '2026-11-23T05:36:44Z'
updated_at: '2026-11-23T05:36:44Z'
adopted_body_hash: null
links:
- relation: relates
  target_type: note
  target_id: n-agli-dec-widen
- relation: relates
  target_type: note
  target_id: n-agli-src-review-20261115
- relation: relates
  target_type: ticket
  target_id: 20
- relation: relates
  target_type: ticket
  target_id: 28
- relation: relates
  target_type: ticket
  target_id: 5
- relation: relates
  target_type: ticket
  target_id: 84
---
## Owner direction — 2026-11-15

The owner requested a careful reconciliation of the de-identification design for audit findings A01 and A02. This is a design question, not approval of the audit's proposed implementation. The observed information paths remain evidence; the intended treatment of safe abstractions and personal growth requires review.

## Existing contract

DESIGN section 2b separates namespace, disclosure scope and processing policy. A namespace is not an access grant. Material may enter a conversation only when the audience is permitted, and its processor must independently satisfy the processing policy.

DESIGN section 2d allows automatic extraction of generalized SelfOnly lessons for solo thinking and personal projects. Removing names alone is insufficient: times, relationships, unusual events and combinations of clues may identify someone. A derived artifact that widens disclosure must be separately stored, retain provenance and a de-identification explanation, and pass review. Restricted source text must not be reintroduced when using the derived artifact across scopes. Existing accepted decision: n-agli-dec-widen.

DESIGN section 6 also permits bounded L2 changes to personality, voice and interests. Preserve this ability to learn. Do not assume that every influence from a private experience is itself a disclosure.

## Implementation evidence to explain

A01: weekly growth reads private journals and generates free-text voice notes and interests. Length/count checks do not establish de-identification. These fields later enter audience-independent L1 without explicit disclosure metadata or a widening review.

A02: solo decisions may use commitments from different private conversations and SelfOnly lessons. A message action carries the free-text reason into the owner's chat context. Choosing an authorized recipient does not establish that every detail in that reason is disclosable to that recipient.

These observations do not establish that every numeric personality delta, abstract interest or generic contact reason leaks information. State and test the intended boundary.

## Questions to resolve

1. Which outputs are safe changes of disposition rather than disclosures? Distinguish bounded numeric/style changes, abstract lessons, free-text voice notes, autobiographical statements and concrete reasons for contacting someone.
2. What makes an abstraction sufficiently de-identified? Define positive and negative examples, including indirect identification and combinations of ordinary details.
3. When may a derived artifact receive broader scope or an independent processing policy? De-identification must not silently grant either. Equally, do not assume that approved public knowledge must inherit every restriction of a private source forever without an explicit decision.
4. How should mixed-source journals and growth artifacts retain sufficient provenance? A reference must not cause restricted source text to be loaded into an outward context.
5. Which transformations remain automatic, which need model review, and which require owner escalation? Do not add owner approval to every harmless style change.
6. For outward initiative, should target-safe wording be regenerated from permitted material, use reviewed abstractions, or follow another explicit boundary? Define behavior before selecting the mechanism.

## Review completion

Deliver a compact source -> transformation -> artifact -> audience/processor matrix, permitted/prohibited examples and synthetic tests proving that useful abstract growth still works while identifying private details cannot enter another audience's model context. Test disclosure and processor permission separately. Record the accepted design before commissioning core changes.

Sources: docs/DESIGN.md sections 2b/2d/6; docs/reviews/2026-11-15/REVIEW.md A01/A02; src/mind/{growth,reflect,decide}.rs; src/agli.rs think/reach_out. This note does not authorize a specific implementation, configuration change or production remediation.

## Candidate cases for the 15 November review

Discussion material for the existing **open** design question; no decision or implementation authorization. Based on A01/A02 and `n-agli-q-growth-deidentification`. The rows below propose distinguishable contracts so review can preserve useful growth while checking disclosure and processor permission independently.

| Source → transformation → artifact | Candidate audience boundary | Candidate processor boundary |
|---|---|---|
| Private episodes → mixed journal → narrative | Private reflection artifact; excluded from outward contexts unless a separately reviewed derivative permits them | Retain source-policy intersection through storage/reload; absence of a local model cannot silently relax it |
| Journal → bounded numeric delta or finite style enum → disposition | Candidate automatic global influence if it carries no source identifiers, quotations, event descriptors or arbitrary text; avoid approval for each harmless change | Processing the source still requires its policy; whether the resulting constrained value receives independent policy needs an explicit rule |
| Journal → free-text voice note/interest → L1 overlay | Treat as potentially disclosive; retain restricted scope until review grants a broader derivative scope | Independent policy only by an explicit reviewed assignment; de-identification alone grants neither scope nor remote processing |
| Private experience → generalized lesson → SelfOnly lesson | Automatic solo use remains possible; outward use requires a separate reviewed abstraction | Solo extraction/use respects source restrictions; broader derivative policy is separately recorded |
| Commitments/lessons → solo decision → contact intent | Keep internal reason private; candidate outward interface carries a recipient and action enum, or a reviewed derivative reference | Generate outward wording using target-permitted inputs and their processor-policy intersection; references must not reload restricted originals |

## Synthetic acceptance examples

Use synthetic Alice/Bob, stub transformations and captured prompts, including stored-artifact reload and provider changes. Assert at the receiving **model input**, not only the final answer.

- Positive growth: Alice-only journal produces an allowed capped agreeableness delta and finite “gentler” style. Group behavior can reflect that change without Alice, her event, or source prose entering L1. Ordinary growth must still execute without per-change owner approval under the chosen automatic rule.
- Direct disclosure: stub returns “be gentle because Alice starts treatment Friday” as voice note/interest. Unapproved group and owner contexts must exclude it.
- Indirect disclosure: remove Alice’s name but retain her unique clinic, date and group role. Also test two individually ordinary artifacts whose combination identifies her; neither should gain wider scope solely through name removal.
- Reviewed abstraction: approved “ask before offering sensitive advice” enters a permitted outward context using only derivative text; captured input excludes its private evidence/provenance payload.
- Initiative: Alice-only commitment causes contact intent. Owner wording must not contain its medical canary; generic contact remains possible.
- Processor separation: permitted audience plus LocalOnly source/derivative still rejects a remote processor. Broader audience approval without processor approval changes only disclosure. Reload without a local provider must preserve this distinction.

## Owner choices still open

Choose the automatic disposition vocabulary; reviewer/owner escalation criteria; whether contextual identifiability requires combined-artifact review; derivative-policy independence and its authority; metadata granularity for mixed journals; and whether outward initiative uses target-safe regeneration, approved abstractions, or another explicit interface. Define rejection/fallback behavior without assuming every influence is a disclosure or every derivative must inherit all restrictions forever.