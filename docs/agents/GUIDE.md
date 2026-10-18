# Agent operating guide (Paperclip team, M1)

Every LabSetup task in Paperclip links here. Read it fully before starting.

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

## Reporting (in the Paperclip issue)

When you finish, comment with:

1. Branch and tip commit.
2. Every gate you ran, with the result (real output, not intentions).
3. Each acceptance criterion: met or not, with evidence (command output or file path).
4. Deviations from the spec.
5. Anything blocked.

Then mark the issue done (or blocked, with the reason).

## Reviews

Review issues are **adversarial**: assume the work is incomplete or wrong until proven otherwise. Re-run the gates yourself in the task's worktree. Check every acceptance criterion against real evidence, file ownership (`git diff m1...m1-<ID>`), interfaces exactly as the spec defines them (later tasks code against them), and correctness: logic bugs, unhandled edge cases the spec calls out, panics on user data, `unwrap`/`expect` on file contents, missing tests. Do not edit the task's files.

- **Pass:** no blocker or major issues. Comment the verdict on the review issue and mark it done. That unblocks the merge.
- **Fail:** comment each blocker or major issue (with `file:line` and evidence) **on the implementation issue** and set that issue back to `todo`. The comment wakes its engineer. Leave your review issue open (`in_review`), not done.
- **Engineer, after fixing:** commit, mark the implementation issue done, then comment on the review issue "Fixed in `<commit>`, please re-review". That comment wakes the reviewer, who reviews again from the top.
- After three failed rounds, the reviewer sets the review issue to `blocked` and comments a summary for the Lead Engineer and the board.

## Merges (Lead Engineer only)

In `D:\Try\LabSetup` on `m1` (check it is clean): `git merge --no-ff m1-<ID>` for each reviewed task of the wave, with message `Merge <ID>: <title>` plus the trailer. Do not merge `m1-T6-spike`; T6b takes `dialogs.rs` from it. Run the gates spec §8.2 requires at that point, and `scripts/check-offline.ps1` whenever `lab-app` compiles. Fix only mechanical integration mismatches (commit `Integration fix: ...`). Then `git worktree remove D:/Try/LabSetup-wt/<ID>` (keep the branch).
