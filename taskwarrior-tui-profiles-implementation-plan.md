# Taskwarrior TUI Profiles Implementation Plan

## Summary

Add first-class named profiles to `taskwarrior-tui`. Each profile selects an independent Taskwarrior configuration and data directory. Users can choose a profile at startup and switch profiles from inside the TUI without mixing data between profiles.

The initial use case is a strict personal/work boundary:

- `personal` uses a database configured to sync with a private TaskChampion server.
- `work` uses a separate database with no personal sync configuration.

Profile switching must restart the TUI against the selected database rather than attempting project-filtered synchronization or reusing state loaded from another profile.

## Goals

- Define named profiles outside any individual Taskwarrior database.
- Select a profile with `taskwarrior-tui --profile <name>`.
- Choose a default profile.
- Display the active profile prominently in the TUI.
- Switch profiles from an in-app picker.
- Preserve the existing `--taskrc`, `--taskdata`, `TASKRC`, and `TASKDATA` behavior.
- Guarantee that all Taskwarrior subprocesses run against exactly one selected profile.
- Keep profile configuration free of Taskwarrior sync secrets.

## Non-goals

- Selective synchronization by project, tag, context, or report.
- Combining tasks from multiple databases into one report.
- Moving tasks between profiles.
- Managing TaskChampion credentials or invoking `task sync` automatically.
- Changing Taskwarrior or TaskChampion synchronization semantics.
- Reloading a different Taskwarrior database inside the existing process.

## Security invariants

1. Personal and work tasks remain in separate Taskwarrior data directories.
2. A profile switch never carries task rows, UUID selections, undo state, reports, contexts, subprocesses, or cached configuration into the next profile.
3. The active profile is always visible before the user creates or modifies a task.
4. Unknown profiles and invalid paths fail closed; they never fall back silently to another database.
5. `--profile` cannot be combined ambiguously with `--taskrc` or `--taskdata`.
6. Profile configuration stores paths and presentation metadata only. Sync secrets remain in protected Taskwarrior configuration includes.
7. Tests use distinct temporary databases and prove that mutations affect only the selected database.

## User experience

### Configuration

Use the currently unused TUI configuration directory for a standalone profile file:

```toml
# ~/.config/taskwarrior-tui/profiles.toml

default = "work"

[profiles.work]
taskrc = "~/.config/task/work.taskrc"
taskdata = "~/.local/share/task/work"
label = "Work"
color = "yellow"

[profiles.personal]
taskrc = "~/.config/task/personal.taskrc"
taskdata = "~/.local/share/task/personal"
label = "Personal"
color = "green"
```

The default config directory is `${XDG_CONFIG_HOME:-~/.config}/taskwarrior-tui`. The existing `--config <FOLDER>` option and `TASKWARRIOR_TUI_CONFIG` environment variable override it.

`taskrc` and `taskdata` paths support `~` expansion. Both should be required in the first release to make the database boundary explicit.

### CLI

Add:

```text
-p, --profile <NAME>    Select a configured Taskwarrior profile
    --list-profiles     List configured profiles and exit
```

Examples:

```bash
taskwarrior-tui --profile work
taskwarrior-tui --profile personal
taskwarrior-tui --list-profiles
```

Resolution precedence:

1. Explicit `--profile`.
2. `default` from `profiles.toml`.
3. Existing legacy behavior when no profiles file exists.

For backward compatibility, `--taskrc` and `--taskdata` continue to work when `--profile` is absent. Clap should reject `--profile` combined with either direct path option.

Environment variables must not unexpectedly override an explicit profile. After resolving a profile, its paths become the authoritative `TASKRC` and `TASKDATA` values for the new process.

### TUI

- Show the active profile in the navigation bar, for example `[Work]`.
- Add a configurable `profile-menu` key, defaulting to `p`.
- The profile picker lists each profile's label and name and marks the current profile.
- Selecting the active profile closes the picker without restarting.
- Selecting another profile exits the event loop, restores the terminal, and restarts the executable with the selected profile.
- `Esc` closes the picker without changing profiles.
- The help popup documents the profile key.

The optional profile color applies only to the profile badge. It must not override profile-specific Taskwarrior colors.

## Technical design

### 1. Profile model and loader

Add `src/profile.rs` containing serializable configuration types and resolution logic:

```rust
struct ProfilesConfig {
    default: Option<String>,
    profiles: BTreeMap<String, Profile>,
}

struct Profile {
    taskrc: PathBuf,
    taskdata: PathBuf,
    label: Option<String>,
    color: Option<ProfileColor>,
}

struct ActiveProfile {
    name: String,
    taskrc: PathBuf,
    taskdata: PathBuf,
    label: String,
    color: Option<ProfileColor>,
}
```

Responsibilities:

- Locate and parse `profiles.toml`.
- Validate profile names, required fields, duplicate names, default selection, and colors.
- Expand `~`, clean paths, and convert them to absolute paths before starting the Tokio runtime.
- Require `taskrc` to be a readable file.
- Allow `taskdata` to be created by Taskwarrior, but require its parent directory to exist or be creatable.
- Produce errors that include the profile name and invalid field without exposing Taskwarrior configuration contents.

Add a TOML parser dependency compatible with the project's existing Serde version.

### 2. CLI resolution

Extend `src/cli.rs` with `--profile` and `--list-profiles`. Mark `--profile` as conflicting with `--taskrc` and `--taskdata`.

Refactor `src/main.rs` so all startup selection happens before logging and before the Tokio runtime starts:

1. Parse CLI arguments.
2. Resolve the TUI config directory.
3. Load profiles when `profiles.toml` exists.
4. Resolve the active profile.
5. Set authoritative `TASKRC` and `TASKDATA` values.
6. Initialize logging and run the TUI.

Do not use the current `set_env_path_if_unset` behavior for a resolved profile. An explicit profile must override inherited `TASKRC` and `TASKDATA`. Preserve the existing helper for legacy direct-path behavior unless changing it is separately justified.

### 3. Application exit contract

Replace the boolean-only quit result with an explicit exit reason:

```rust
enum ExitReason {
    Quit,
    SwitchProfile(String),
}
```

Update `TaskwarriorTui::run` to return `Result<ExitReason>`. Store the active profile and pending exit reason on `TaskwarriorTui`, or pass profile metadata through a small application context.

Normal `q` and `Ctrl-C` return `ExitReason::Quit`. Choosing another profile returns `ExitReason::SwitchProfile(name)`.

### 4. Profile picker

Add `Action::ProfileMenu` and a small profile-menu state modeled after the existing context and report menus. It should support:

- Up/down navigation.
- Search by profile name or label if the existing menu abstraction makes this inexpensive.
- Selection with the standard select key.
- Cancellation with `Esc` or the configured quit key.

Add `profile_menu` to `KeyConfig`:

```text
uda.taskwarrior-tui.keyconfig.profile-menu=p
```

Although profiles live outside `.taskrc`, retaining the keybinding in Taskwarrior configuration preserves the existing TUI customization model. The profile picker should remain reachable in every configured profile.

### 5. Active-profile presentation

Pass `ActiveProfile` into `TaskwarriorTui::new` and add a profile badge to the existing navigation bar in `TaskwarriorTui::draw_navbar`.

When running without a profiles file, preserve the current navigation bar rather than inventing a misleading profile name. Optionally display `Custom` when direct `--taskrc` or `--taskdata` arguments were supplied, but this can be deferred.

### 6. Safe process restart

Perform the restart only after `pause_tui` has restored terminal state and the async runtime has returned control to `main`.

Build the child invocation with `std::env::current_exe()` and preserve relevant options such as the selected report and TUI data/config directories. Remove inherited `TASKRC` and `TASKDATA`, then set or pass the selected profile explicitly.

Platform strategy:

- Unix: use `std::os::unix::process::CommandExt::exec` after terminal restoration so repeated switches do not create a chain of waiting parent processes.
- Windows: spawn the replacement process and exit the current process after successful spawn.

If restart fails, print a normal terminal error containing the requested profile and executable path. Never resume the old TUI while claiming that the new profile is active.

A later refactor could attach profile environment variables to every Taskwarrior `Command`, but that touches many synchronous, asynchronous, and background subprocess call sites. Process restart is a smaller and safer first implementation.

### 7. Documentation and completions

Update:

- `README.md` configuration and launch examples.
- `docs/src/content/docs/configuration/advanced.md` with the full profile schema and precedence rules.
- `docs/src/content/docs/quick_start.md` with profile selection.
- Generated shell completions and the man page for new CLI options.
- Help template for the profile-menu key.

Document clearly that profiles isolate databases; they do not filter Taskwarrior synchronization by project.

## Delivery phases

### Phase 1: Configuration and CLI

- Add profile types, TOML parsing, path resolution, and validation.
- Add `--profile` and `--list-profiles`.
- Launch directly into the selected profile.
- Preserve legacy behavior when no profile configuration exists.

Deliverable: users can replace shell wrappers with `taskwarrior-tui --profile <name>`.

### Phase 2: Visibility

- Pass active-profile metadata into the app.
- Render the profile badge.
- Add help and documentation.

Deliverable: users can always verify the active confidentiality boundary.

### Phase 3: In-app switching

- Add the keybinding and profile picker.
- Introduce `ExitReason`.
- Implement terminal-safe process restart.

Deliverable: users can switch profiles without returning manually to the shell.

### Phase 4: Hardening and upstreaming

- Add cross-platform restart tests where practical.
- Test path expansion and inherited environment conflicts.
- Update completions and packaging.
- Open an upstream PR, keeping commits separated by the phases above.

## Test plan

### Unit tests

- Parse a valid profiles file.
- Reject an unknown default profile.
- Reject missing `taskrc` or `taskdata`.
- Reject unknown colors and malformed TOML.
- Expand home-relative and relative paths deterministically.
- Resolve CLI profile precedence.
- Reject `--profile` with direct Taskwarrior path options.
- Ensure an explicit profile overrides inherited `TASKRC` and `TASKDATA`.
- Return the correct `ExitReason` from profile selection.

### Integration tests

Create temporary `work` and `personal` taskrc/data pairs:

1. Add a distinct seed task to each database.
2. Launch profile resolution for `work` and confirm only the work task is exported.
3. Launch profile resolution for `personal` and confirm only the personal task is exported.
4. Add a task through each selected profile and verify it appears in only that database.
5. Configure sync only in personal and verify work diagnostics/configuration do not contain the personal sync URL.
6. Simulate a switch and verify the replacement command removes inherited profile variables and selects the target profile.
7. Verify missing or unreadable profile configuration exits non-zero without starting the TUI.

Do not run synchronization against a real server in automated tests.

### Manual tests

- Start with no profiles file and confirm behavior is unchanged.
- Start each named profile on macOS and Linux.
- Switch repeatedly and check for terminal corruption or accumulating processes.
- Switch while filters, selections, a context, and a report menu are active; confirm none carry over.
- Confirm the profile badge remains visible at narrow terminal widths.
- Confirm `q`, `Ctrl-C`, panic handling, and failed restarts restore the terminal.
- Confirm personal `task sync` works and work data never appears on the personal server.

## Acceptance criteria

- `taskwarrior-tui --profile work` and `--profile personal` open their respective databases.
- The active profile is visible during all normal task-list operations.
- A user can switch profiles from inside the TUI with a configurable key.
- Switching creates a clean application instance with no prior-profile state.
- Work and personal mutations remain isolated in integration tests.
- Existing users without `profiles.toml` see no behavior change.
- Existing `--taskrc`, `--taskdata`, `TASKRC`, and `TASKDATA` workflows remain supported.
- Invalid profile configuration fails with actionable errors and no silent fallback.
