# First Draft: build everything, then test (board decision, 2026-10-20)

Goal: a **fully functional prototype** of every remaining milestone in DESIGN.md §11 (M2–M5) before any test-and-debug cycle. The old loop (build → test → find flaw → fix → test again, per task) burned most of the tokens. In the First Draft each task is built **once**, checked **once**, and the team moves on. Testing, review and fixing happen together at the end of M5 (**Hardening**), after every feature exists.

This file overrides GUIDE.md wherever they disagree, for issues in the **LabSetup First Draft** project. Everything else in GUIDE.md (worktrees, build environment, git, Multica basics, failed runs) still applies.

## Milestones (DESIGN.md §11)

| Parent issue | Scope | Exit criteria (§11) |
|---|---|---|
| M2 Library + placement | `lab-catalog`, `lab-parts`, built-in library (F3), room fixtures (F1), platform stacking (F2), drag-drop, hole snap, mating (F5), beam-height stack builder (F6) | Build a 20-component table at 75 mm beam height without typing coordinates |
| M3 Views | Top view, split, isolate plate, measure, animated transitions, selection outline, inspector editing (F7); M1 findings 3–4 | Switch 3D↔Top smoothly; isolate one plate |
| M4 Vendor import | Import flow, LODs, connector wizard, laser-head template (F4); §13 step2glb findings | Import a KM100 + one laser head and mount them correctly |
| M5 Polish & ship | Features: design-system pass, light/dark, command palette, PNG + BOM export, embedded parts in the project file, MSI installer. Then **Hardening**: performance pass, tests, audit, fixes | Meets the §1 non-functional targets on the lab PC |

Each parent has staged sub-issues. Stage 1 is the Lead's **plan** issue; later stages are build tasks. M5's last stages are Hardening. A milestone starts only when the previous one is done.

## Branch

Integration branch **`draft`**, created from `m1` by the first merge. Task worktree: `D:/Try/LabSetup-wt/<KEY>` on branch `draft-<KEY>` from the current `draft`. Engineer Sonnet merges (`git merge --no-ff draft-<KEY>`, message `Merge <KEY>: <title>`) when the Lead asks.

## The one rule: fix now, or log and move on

When you find a flaw, decide once:

**Fix now** only if it is one of:
1. The workspace does not compile.
2. Your own task's main feature does not work at all.
3. It crashes or loses data on the main path that the next stage builds on.

Fix it in the same run, check it once, move on.

**Everything else is deferred:** edge cases, visual polish, slow paths, missing tests, other tasks' bugs, clippy lints, and anything outside your issue. Add **one line** (`<file:line or screen> · what is wrong · how to reproduce`) as a comment on the M5 issue **"Defect log"**. Do not fix it, do not investigate further, do not reopen other issues.

**Retry cap:** at most **2** attempts at any one failing build, command or fix. After the second failure, add it to the defect log, set your issue `blocked` with the error, and stop. Never retry the same command unchanged.

## Engineers

- Read your milestone's plan (`docs/M<n>-PLAN.md` on `draft`) and your issue. Open only the files your task touches; do not re-read the whole spec or code base.
- Build the whole feature in one pass, including its UI wiring, so a user can use it.
- **Check once:** `cargo build --package lab-app --offline` passes, and the feature works once on its happy path (launch the app, or one unit test for pure logic such as the solver or snap math). No test suites, benchmarks or screenshots; Hardening does that.
- Do not refactor, rename or tidy code you were not asked to change.
- Report in **five lines or fewer**: branch and tip commit, the one check and its result, what is not done, defect-log lines you added. End the report with `@Lead Engineer` so the Lead knows it is in. Set the issue `done` (or `blocked`, with the reason) and stop.

## Lead Engineer

- **Plan issue (stage 1 of each milestone):** write `docs/M<n>-PLAN.md` on `draft`: for each build task, the files it owns, the interfaces other tasks depend on (exact types and function signatures), and what "works" means. Keep it short; engineers implement from it. The engineer on each issue is a default sized to the job; reassign if the plan shows it is harder or easier. Commit the plan and set the plan issue `done`.
- **Woken by a report @-mention:** if other issues of the same stage are still open, stop without commenting. If the report says `blocked`, decide now (see 'Lead decides alone'). If it was the stage's last issue, go on as below.
- **A build stage finished:** read the reports only. @-mention Engineer Sonnet on the parent to merge the stage's branches into `draft` and run `cargo build --workspace --offline` once; when it reports success, set the next stage to `todo`. Do not run gates, open the app or review code.
- **A milestone's last stage finished:** set the parent `done` and set stage 1 of the next milestone to `todo`.
- **Release and push per milestone** (board request, 2026-10-22): when a milestone (or fix round) finishes, Engineer Sonnet builds `cargo build --package lab-app --release --offline`, copies the exe + dxcompiler.dll + BUILD.txt to `D:\Try\LabSetup-builds\<tag>\`, tags the commit `<tag>`, and pushes: `git push origin draft --follow-tags`. Never force-push.
- No per-task reviews in the First Draft. Do not reopen a finished task for a deferred defect.

**Lead decides alone** (board decision, 2026-10-20). Never ask the board (the user) a question and never wait for it. Every choice is yours: scope cuts, spec ambiguities, reassignments, blocked work, dropping or postponing defects, moving to the next stage or milestone. Pick the option that best serves DESIGN.md and the milestone's exit criteria, record it in one line on the parent (`Decision: <what> · <why>`), and act. A blocked issue gets a decision: reassign, split, cut scope, or cancel it and log it on the Defect log. Never leave the project waiting; the board reads decisions afterwards and overrides if it disagrees.

**Lead progress check**: wakes you right after each Claude usage-limit reset (the Failed-run sweep schedules it for reset + 2 min on the active milestone parent) and otherwise every 5 hours (autopilot at 01:22, 06:22, 11:22, 16:22, 21:22 Asia/Bangkok), even if no event did. Find the active milestone parent (the First Draft project's lowest unfinished M2–M5). If any of its issues is `in_progress`, or has a run queued or running, stop without commenting. Otherwise do the next step from the bullets above: a finished plan or build stage → next stage; a finished milestone → next milestone; a stuck `blocked` or `todo` issue with no run in the last 2 hours → decide and restart it. After M5 Hardening and the final audit are done, set the M5 parent `done`, comment a one-paragraph summary on it for the board, and stop.

## Hardening (end of M5)

1. **Test and audit (QA Reviewer Sonnet):** run every gate once: `cargo test --workspace --offline`, clippy, `scripts/check-offline.ps1`, the §12 end-to-end walkthrough (Rb-87 MOT + SPDC table → save → reopen → top view of the isolated plate → PNG and BOM export), and the 2k/10k-part performance scenes against §1. Merge the findings with the defect log into one ranked list. Fix nothing.
2. **Plan fixes (Lead):** group the list into fix batches by area (at most about ten issues), each with its defects and engineer. Drop or postpone what does not matter for a first release.
3. **Fix batches (engineers):** fix everything in the batch, run that area's tests once, report. Same 2-attempt cap.
4. **Regression and measure (Engineer Sonnet):** one full gate run and the §1 measurements; record results in DESIGN.md. Anything still failing goes to the Lead, who decides to fix, postpone or accept it; not into another loop.
5. **Final audit (Lead).**
