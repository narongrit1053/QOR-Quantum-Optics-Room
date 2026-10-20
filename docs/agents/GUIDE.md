# Agent operating guide (Multica team, M1)

Every LabSetup issue in Multica links here. Read it fully before starting.

**First Draft (M2–M5, from 2026-10-20):** issues in the *LabSetup First Draft* project follow [FIRST-DRAFT.md](FIRST-DRAFT.md), which overrides this guide where they disagree: build every feature once, fix only blockers, log everything else, test at the end.

The work moved from Paperclip to Multica on 2026-10-19. Each Multica issue carries the history of its Paperclip predecessor (LAB-n) in its first comment.

## The work

- `docs/M1-SPEC.md` is the final, API-verified M1 specification. Its appendix records the user's answers. Treat it as binding.
- `docs/m1-tasks.json` holds every task record: owned files, dependencies, interfaces, acceptance criteria.
- `docs/DESIGN.md` is the overall design. `README.md` explains building and running.
- The integration branch is **`m1`** in the main checkout `D:\Try\LabSetup`. **Never edit files in the main checkout** unless your issue is a merge issue.

## Worktrees: one per task

Each task `<ID>` (for example `T1`) is built in its own worktree on branch `m1-<ID>`:

```bash
git -C D:/Try/LabSetup worktree add D:/Try/LabSetup-wt/<ID> -b m1-<ID> m1
```

If `D:/Try/LabSetup-wt/<ID>` already exists, **reuse it and continue from its state**. Earlier runs were interrupted by usage limits and left partial work there, sometimes uncommitted. Inspect `git status` and `git log m1..HEAD` first, and keep what is good.

Touch only the files your task owns. If the spec is wrong or impossible, make the smallest deviation that keeps its intent and interfaces, and record it. If you cannot finish without editing another task's files, stop and say so in the issue.

## Build environment (Git Bash)

```bash
export CARGO_HOME=/d/dev/cargo RUSTUP_HOME=/d/dev/rustup PATH=/d/dev/cargo/bin:$PATH CARGO_TARGET_DIR=D:/Try/LabSetup/target
```

- The target dir is **shared on purpose**: a fresh one rebuilds Bevy and Skia for about an hour. If cargo prints "Blocking waiting for file lock", wait.
- Always pass `--offline`. Builds that link Bevy or Slint take minutes; allow long timeouts.
- Never change the Bevy feature list or any dependency's features, and never run `cargo update`.
- Test gates are **per crate** (spec §8.2). The workspace is expected not to compile between T2 and T6c.

## Checking the running app

Helpers are in `scripts/dev` (see its README): `shot.ps1` captures the window, `post.ps1` posts mouse input, `idle.ps1` measures idle CPU/GPU. Several agents may run LabSetup at once:

- Start it with `Start-Process ... -PassThru` and act only on **your** process id. Stop only that process.
- `target/debug/labsetup.exe` is shared and overwritten by other builds. Copy it right after your build to a unique name in your worktree (for example `labsetup-<ID>.exe`), run that copy, and delete it before committing.
- Never move the real mouse cursor or touch other windows. The user may be working on this PC.
- Save screenshots and measurements under `D:/Try/LabSetup-wt/evidence/<ID>/`.

## Git

- Commit messages end with a blank line and `Co-Authored-By: <your model> <noreply@anthropic.com or your provider's address>`.
- Stage paths explicitly; never `git add -A`. Commit only files your task owns.
- Never push, never rewrite history on `m1`, never delete branches.

## Multica basics

- Use the `multica` CLI for everything: `multica issue get <KEY>`, `multica issue comment add <KEY> --content-file <file>` (use `--content-file` for multi-line text on Windows), `multica issue update <KEY> --status <status>`.
- Statuses: `backlog`, `todo`, `in_progress`, `in_review`, `done`, `blocked`, `cancelled`. Set them yourself; the server does not change them when a run starts or ends.
- If your run was started by a comment, reply **under that comment** (`--parent <comment-id>`); a top-level comment on that issue is rejected.
- To hand work to another agent, **@-mention it by name** in your comment (for example `@QA Reviewer Sonnet`). The mention starts a run for that agent. Say exactly which issue it should act on.
- M1 is one parent issue with staged sub-issues. Later stages wait in `backlog`; the Lead Engineer promotes them.

## Reporting

When you finish, comment on your issue with:

1. Branch and tip commit.
2. Every gate you ran, with the result (real output, not intentions).
3. Each acceptance criterion: met or not, with evidence (command output or file path).
4. Deviations from the spec.
5. Anything blocked.

End the comment with `@Lead Engineer` so the Lead knows the report is in. Then set the issue to `done` (or `blocked`, with the reason).

## Reviews

**Final audit (stage 9).** The task is already merged into `m1`, and later tasks build on it. Review it as merged: create a review worktree `git -C D:/Try/LabSetup worktree add --detach D:/Try/LabSetup-wt/review-<ID> m1`, run the task's gates there, and read the task's change with `git log --oneline --grep "<ID>" m1` and `git diff <merge>^1 <merge>` for its merge commit (plus any `Integration fix` commits touching its files). Check its interfaces against how later tasks actually use them. Remove the review worktree when done.

**Fixes during the audit** go on a new branch from the current `m1`: `git -C D:/Try/LabSetup worktree add D:/Try/LabSetup-wt/<ID>-fix -b m1-<ID>-fix m1`, touching only the task's owned files. The reviewer re-reviews that branch; the Lead merges it after the audit.

Review issues are **adversarial**: assume the work is incomplete or wrong until proven otherwise. Re-run the gates yourself in the task's worktree. Check every acceptance criterion against real evidence, file ownership (`git diff m1...m1-<ID>`), interfaces exactly as the spec defines them (later tasks code against them), and correctness: logic bugs, unhandled edge cases the spec calls out, panics on user data, `unwrap`/`expect` on file contents, missing tests. Do not edit the task's files.

- **Pass:** no blocker or major issues. Comment the verdict on the review issue and set it `done`.
- **Fail:** comment each blocker or major issue (with `file:line` and evidence) on the **implementation** issue, @-mentioning its engineer, and set that issue back to `todo`. Reopening a `done` issue also needs its failure wakeup again; see the end of 'Failed runs and usage limits'. Set your review issue to `in_review` (not done).
- **Engineer, after fixing:** commit, set the implementation issue `done`, and comment there @-mentioning the reviewer: "Fixed in `<commit>`, please re-review <review issue key>". The reviewer reviews again from the top and updates the review issue.
- After three failed rounds, the reviewer sets the review issue `blocked` and @-mentions the Lead Engineer with a summary.

## Lead Engineer: running the stages

You own the parent issue "M1". Multica wakes you when every sub-issue in the earliest unfinished stage is `done` or `cancelled`.

**Build first, audit last** (board decision, 2026-10-19). Waves 1 and 2 (T1, T4a, T5, T6-spike, T2) were reviewed and merged the old way. From stage 5 on, every remaining feature is built and merged first, and all the remaining reviews run together at the end:

| Stage | Issues |
|---|---|
| 5 | T3, T4b |
| 6 | T4c, T6a |
| 7 | T6c |
| 8 | T6b |
| 9 | Reviews of T3, T4b, T4c, T6a, T6c, T6b (final audit, in parallel) |
| 10 | T7 (yours) |

When woken:

1. **A build stage (5-8) finished:** read each task's report. If a report shows a failed or missing gate, comment that on the issue @-mentioning its engineer and set it `todo`. Otherwise @-mention **Engineer Sonnet** on the parent with the exact list of branches to merge (Merges, below); it re-runs the gates, merges, and reports back. When its report is clean, set the next stage to `todo`. After stage 8, set all stage-9 reviews to `todo`.
2. **A build issue finished again during the audit** (a review fix): do nothing; the fix is merged after the audit.
3. **Stage 9 finished:** @-mention **Engineer Sonnet** to merge every `m1-<ID>-fix` branch the audit produced (Merges) and run the full §8.2 gates; when its report is clean, set T7 to `todo`.
4. Comment on the parent what you did. If something is blocked, decide the way forward yourself (see 'Lead decides alone' in FIRST-DRAFT.md); do not wait for the board.

## Failed runs and usage limits

**Team** (board decision, 2026-10-19, latest revision): all labor runs on Claude, sized to the job. The **Lead Engineer** (Opus high) manages the project, writes detailed plans and does the final audit; it writes no code, runs no build-stage gates and does no merges. When it assigns a task it picks the cheapest engineer that can do it:

- **Engineer Opus** (Opus medium): complex work: new subsystems, tricky geometry, Bevy/Slint integration, hard bugs.
- **Engineer Opus Low** (Opus low): moderate work and review fix rounds.
- **Engineer Sonnet** (Sonnet medium): simple and mechanical work: small fixes, merges, gates, integration fixes.
- **QA Reviewer Sonnet** (Sonnet high): every review.

**Limit Dispatcher** stays on Antigravity (Gemini 3.8 Flash High) so failures can be triaged when the Claude plan is out. **Mika** (Claude) is the workspace Chief of Staff. Gemini Engineer Flash only finishes POLA-28 and takes no new work; every other engineer and reviewer agent is archived, and work that names them goes to the agents above.

Quotas: all Claude agents share one Claude plan; all Antigravity agents share one Flash quota. Every M1 issue has a `task.failed` wakeup that wakes a **triager on the other provider**, because the failed agent's own quota may be gone:

**Automatic sweep** (board decision, 2026-10-20): the *Failed-run sweep* autopilot runs the Limit Dispatcher every 15 minutes on its own. It finds failed runs of Claude agents from the last 20 minutes and triages them by the steps below, skipping any issue that already has a later run or a pending wakeup. Nobody needs to register failure wakeups any more; existing ones still fire and the sweep backs them up.

- Issues assigned to Antigravity agents (Gemini Engineer Flash): **Lead Engineer** triages.
- Issues assigned to Claude agents (Lead Engineer, Engineer\*, QA Reviewer Sonnet) and the M1 parent: **Limit Dispatcher** (Flash) triages.

When woken by a failure, the triager does only this (no code, no worktree changes, no other issues):

1. Read the error: `multica issue runs <KEY>`, then `multica issue run-messages <run-id>` for the failed run. If the latest run did not fail (it completed, or a later run is queued or running), comment that and stop.
2. **Usage or rate limit** (429, "quota", "usage limit", "rate limit", "resource exhausted", "try again at ..."):
   - Any agent: **schedule a rerun after the reset** for the same assignee: `multica issue wakeup create <KEY> --kind at --after <wait> --agent-id <assignee id> --instruction "Usage limit has reset. Continue this issue from its worktree and comments; do not start over."` Use the reset time from the error plus 2 minutes; if there is none, `5h`.
3. **Any other failure:** if the previous run on this issue did not fail the same way, `multica issue rerun <KEY>`. If it did, set the issue `blocked` and @-mention the Lead Engineer (or, if the Lead is the assignee, rerun it once more after 1h; the Lead's progress check picks it up after that).
4. Comment once on the issue: the error (one line) and what you did.

Agent ids for `--agent-id`: Lead Engineer `a0cfce75-c036-458e-8b6a-ae1991d7c036`, Engineer Opus `c560211e-a30f-45de-8ed0-80e94df9c26b`, Engineer Opus Low `9bffb221-f102-43a0-a10a-f28e39ed5144`, Engineer Sonnet `1a8be1b5-3ca9-42ca-8e19-02b539b5826d`, QA Reviewer Sonnet `08ecefee-2d76-424c-a1e5-565b9427f347`, Gemini Engineer Flash `dcc3bd63-406b-4d84-bebd-18eff80d128e`, Limit Dispatcher `2fd0382f-1a6c-420d-ba2f-36bcef45d688`.

Closed (`done`/`cancelled`) issues cannot hold wakeups; the automatic sweep covers reopened issues, so no wakeup needs to be registered.

The platform pauses a wakeup rule after it fires three times in a chain with no person in between, so an issue that keeps failing ends up waiting for the board.

## Merges (Engineer Sonnet, when the Lead asks)

Only merge the branches the Lead Engineer listed. In `D:\Try\LabSetup` on `m1` (check it is clean): `git merge --no-ff m1-<ID>` for each reviewed task of the wave, with message `Merge <ID>: <title>` plus the trailer. Do not merge `m1-T6-spike`; T6b takes `dialogs.rs` from it. Run the gates spec §8.2 requires at that point, and `scripts/check-offline.ps1` whenever `lab-app` compiles. Fix only mechanical integration mismatches (commit `Integration fix: ...`). Then `git worktree remove D:/Try/LabSetup-wt/<ID>` (keep the branch). Report each merge commit and gate result on the parent, @-mentioning the Lead Engineer.
