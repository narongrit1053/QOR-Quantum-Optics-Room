# Agent operating guide (Multica team, M1)

Every LabSetup issue in Multica links here. Read it fully before starting.

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
- To hand work to another agent, **@-mention it by name** in your comment (for example `@QA Reviewer`). The mention starts a run for that agent. Say exactly which issue it should act on.
- M1 is one parent issue with staged sub-issues. Later stages wait in `backlog`; the Lead Engineer promotes them.

## Reporting

When you finish, comment on your issue with:

1. Branch and tip commit.
2. Every gate you ran, with the result (real output, not intentions).
3. Each acceptance criterion: met or not, with evidence (command output or file path).
4. Deviations from the spec.
5. Anything blocked.

Then set the issue to `done` (or `blocked`, with the reason).

## Reviews

Review issues are **adversarial**: assume the work is incomplete or wrong until proven otherwise. Re-run the gates yourself in the task's worktree. Check every acceptance criterion against real evidence, file ownership (`git diff m1...m1-<ID>`), interfaces exactly as the spec defines them (later tasks code against them), and correctness: logic bugs, unhandled edge cases the spec calls out, panics on user data, `unwrap`/`expect` on file contents, missing tests. Do not edit the task's files.

- **Pass:** no blocker or major issues. Comment the verdict on the review issue and set it `done`.
- **Fail:** comment each blocker or major issue (with `file:line` and evidence) on the **implementation** issue, @-mentioning its engineer, and set that issue back to `todo`. Set your review issue to `in_review` (not done).
- **Engineer, after fixing:** commit, set the implementation issue `done`, and comment there @-mentioning the reviewer: "Fixed in `<commit>`, please re-review <review issue key>". The reviewer reviews again from the top and updates the review issue.
- After three failed rounds, the reviewer sets the review issue `blocked` and @-mentions the Lead Engineer with a summary.

## Lead Engineer: running the stages

You own the parent issue "M1". Multica wakes you when every sub-issue in the earliest unfinished stage is `done` or `cancelled`. Each wave has two stages: implementation, then review. When woken:

1. **An implementation stage finished:** set that wave's review sub-issues to `todo`.
2. **A review stage finished:** merge the wave (below), then set the next wave's implementation sub-issues to `todo`. After the last wave, set "T7" to `todo` (it is yours).
3. Comment on the parent what you did. If something is blocked, set the parent `blocked` and explain; the board (the user) decides.

## Failed runs and usage limits

Quotas: all Claude agents share one Claude plan; all Argon agents share one Argon quota; Flash has its own. Every M1 issue has a `task.failed` wakeup that wakes a **triager on the other provider**, because the failed agent's own quota may be gone:

- Issues assigned to Antigravity agents (Gemini Engineer\*, QA Reviewer\*): **Lead Engineer** triages.
- Issues assigned to Claude agents (Lead Engineer, Rust Engineer\*) and the M1 parent: **Limit Dispatcher** (Flash) triages.

When woken by a failure, the triager does only this (no code, no worktree changes, no other issues):

1. Read the error: `multica issue runs <KEY>`, then `multica issue run-messages <run-id>` for the failed run.
2. **Usage or rate limit** (429, "quota", "usage limit", "rate limit", "resource exhausted", "try again at ..."):
   - Failed agent is **Argon**: reassign to its Flash fallback (engineers to **Gemini Engineer Flash**, reviewers to **QA Reviewer Flash**) with `multica issue update <KEY> --assignee "<name>"`. Exception: the T2 and T4b reviews (POLA-12, POLA-16) never go to Flash; schedule a rerun instead (below).
   - Otherwise (Claude, Flash, or the exception): **schedule a rerun after the reset** for the same assignee: `multica issue wakeup create <KEY> --kind at --after <wait> --agent-id <assignee id> --instruction "Usage limit has reset. Continue this issue from its worktree and comments; do not start over."` Use the reset time from the error plus 10 minutes; if there is none, `5h`.
   - If a reassignment does not start a run within a few minutes, `multica issue rerun <KEY>`.
3. **Any other failure:** if the previous run on this issue did not fail the same way, `multica issue rerun <KEY>`. If it did, set the issue `blocked` and @-mention the Lead Engineer (or, if the Lead is the assignee, leave it for the board).
4. Comment once on the issue: the error (one line) and what you did.

Agent ids for `--agent-id`: Lead Engineer `a0cfce75-c036-458e-8b6a-ae1991d7c036`, Rust Engineer `a2d64798-7e14-42f4-9de6-b369a1633278`, Rust Engineer XHigh `c864e3c5-8f38-466b-b01e-c54254dbd753`, Gemini Engineer `407bed7a-4d2b-4130-a012-a39da2d7d933`, Gemini Engineer Medium `7d8b7572-25aa-41f0-b243-16c986d4f43f`, QA Reviewer `ac9b1599-4d1d-43b7-b7ac-29c94ee1220d`, QA Reviewer Medium `64a587cf-66a5-4a64-a08d-e4b866964ccd`, Gemini Engineer Flash `dcc3bd63-406b-4d84-bebd-18eff80d128e`, QA Reviewer Flash `fffab64a-dab3-4135-bc74-82fe0a5bfcbe`.

The platform pauses a wakeup rule after it fires three times in a chain with no person in between, so an issue that keeps failing ends up waiting for the board.

## Merges (Lead Engineer only)

In `D:\Try\LabSetup` on `m1` (check it is clean): `git merge --no-ff m1-<ID>` for each reviewed task of the wave, with message `Merge <ID>: <title>` plus the trailer. Do not merge `m1-T6-spike`; T6b takes `dialogs.rs` from it. Run the gates spec §8.2 requires at that point, and `scripts/check-offline.ps1` whenever `lab-app` compiles. Fix only mechanical integration mismatches (commit `Integration fix: ...`). Then `git worktree remove D:/Try/LabSetup-wt/<ID>` (keep the branch).
