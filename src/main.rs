#![allow(dead_code)]
#![allow(unused_imports)]
#![allow(unused_variables)]
#![allow(clippy::too_many_arguments)]

mod action;
mod app;
mod calendar;
mod cli;
mod completion;
mod config;
mod datetime;
mod event;
mod help;
mod history;
mod keyconfig;
mod pane;
mod profile;
mod scrollbar;
mod table;
mod task_report;
mod ui;
mod utils;

use std::{
  env,
  error::Error,
  io::{self, Write},
  panic,
  path::{Path, PathBuf},
  time::Duration,
};

use anyhow::{Context, Result, anyhow, bail};
use app::{ExitReason, Mode, TaskwarriorTui};
use crossterm::{
  cursor,
  event::{DisableBracketedPaste, DisableMouseCapture, EnableMouseCapture, EventStream},
  execute,
  terminal::{Clear, ClearType, EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use futures::stream::{FuturesUnordered, StreamExt};
use log::{Level, LevelFilter, debug, error, info, log_enabled, trace, warn};
use log4rs::{
  append::file::FileAppender,
  config::{Appender, Config, Logger, Root},
  encode::pattern::PatternEncoder,
};
use path_clean::PathClean;
use ratatui::{Terminal, backend::CrosstermBackend};

use crate::{
  action::Action,
  event::Event,
  keyconfig::KeyConfig,
  profile::{ProfileMenuState, ProfilesConfig},
};

const LOG_PATTERN: &str = "{d(%Y-%m-%d %H:%M:%S)} | {l} | {f}:{L} | {m}{n}";

pub fn destruct_terminal() {
  disable_raw_mode().unwrap();
  execute!(io::stdout(), LeaveAlternateScreen, DisableMouseCapture, DisableBracketedPaste).unwrap();
  execute!(io::stdout(), cursor::Show).unwrap();
}

pub fn initialize_logging() {
  let data_local_dir = if let Ok(s) = std::env::var("TASKWARRIOR_TUI_DATA") {
    PathBuf::from(s)
  } else {
    dirs::data_local_dir()
      .expect("Unable to find data directory for taskwarrior-tui")
      .join("taskwarrior-tui")
  };

  std::fs::create_dir_all(&data_local_dir).unwrap_or_else(|_| panic!("Unable to create {:?}", data_local_dir));

  let logfile = FileAppender::builder()
    .encoder(Box::new(PatternEncoder::new(LOG_PATTERN)))
    .append(false)
    .build(data_local_dir.join("taskwarrior-tui.log"))
    .expect("Failed to build log file appender.");

  let levelfilter = match std::env::var("TASKWARRIOR_TUI_LOG_LEVEL").unwrap_or_else(|_| "info".to_string()).as_str() {
    "off" => LevelFilter::Off,
    "warn" => LevelFilter::Warn,
    "info" => LevelFilter::Info,
    "debug" => LevelFilter::Debug,
    "trace" => LevelFilter::Trace,
    _ => LevelFilter::Info,
  };
  let config = Config::builder()
    .appender(Appender::builder().build("logfile", Box::new(logfile)))
    .logger(Logger::builder().build("taskwarrior_tui", levelfilter))
    .build(Root::builder().appender("logfile").build(LevelFilter::Info))
    .expect("Failed to build logging config.");

  log4rs::init_config(config).expect("Failed to initialize logging.");
}

pub fn absolute_path(path: impl AsRef<Path>) -> io::Result<PathBuf> {
  let path = path.as_ref();

  let absolute_path = if path.is_absolute() {
    path.to_path_buf()
  } else {
    env::current_dir()?.join(path)
  }
  .clean();

  Ok(absolute_path)
}

fn set_env_path_if_unset(key: &str, value: &str, path_name: &str) {
  if env::var_os(key).is_none() {
    let absolute_path = absolute_path(PathBuf::from(value)).unwrap_or_else(|_| panic!("Unable to get path for {path_name}"));

    // SAFETY: this runs in `main` before the Tokio runtime is created and before
    // any threads are spawned, so there is no concurrent access to the process
    // environment while mutating it.
    unsafe {
      env::set_var(key, absolute_path);
    }
  } else {
    warn!("{key} environment variable cannot be set.")
  }
}

fn config_directory() -> Result<PathBuf> {
  if let Some(path) = env::var_os("TASKWARRIOR_TUI_CONFIG") {
    return absolute_path(PathBuf::from(path)).context("Unable to resolve TASKWARRIOR_TUI_CONFIG");
  }
  let base = if let Some(path) = env::var_os("XDG_CONFIG_HOME") {
    absolute_path(PathBuf::from(path)).context("Unable to resolve XDG_CONFIG_HOME")?
  } else {
    dirs::home_dir()
      .ok_or_else(|| anyhow!("Unable to find the home directory"))?
      .join(".config")
  };
  Ok(base.join("taskwarrior-tui").clean())
}

fn set_profile_environment(profile: &profile::ActiveProfile) {
  // SAFETY: profile selection happens before the Tokio runtime starts, so no
  // other thread can read the process environment concurrently.
  unsafe {
    env::set_var("TASKRC", &profile.taskrc);
    env::set_var("TASKDATA", &profile.taskdata);
  }
}

fn load_profile_menu(config_dir: &Path, requested: Option<&str>) -> Result<Option<ProfileMenuState>> {
  let profiles_path = config_dir.join("profiles.toml");
  if !profiles_path.exists() {
    if let Some(name) = requested {
      bail!(
        "Cannot select profile '{name}': profile configuration was not found at {}",
        profiles_path.display()
      );
    }
    return Ok(None);
  }

  let config = ProfilesConfig::load(&profiles_path)?;
  let selected_name = requested
    .map(str::to_string)
    .or_else(|| config.default.clone())
    .ok_or_else(|| anyhow!("No default profile is configured in {}; use --profile <NAME>", profiles_path.display()))?;
  let active = config.resolve(&selected_name)?;
  let profiles = config.resolved_profiles()?;
  Ok(Some(ProfileMenuState::new(active, profiles)))
}

fn list_profiles(config_dir: &Path) -> Result<()> {
  let profiles_path = config_dir.join("profiles.toml");
  if !profiles_path.exists() {
    println!("No profiles configured at {}", profiles_path.display());
    return Ok(());
  }

  let config = ProfilesConfig::load(&profiles_path)?;
  println!("NAME\tLABEL\tDEFAULT");
  for profile in config.resolved_profiles()? {
    let default = if config.default.as_deref() == Some(profile.name.as_str()) {
      "yes"
    } else {
      ""
    };
    println!("{}\t{}\t{}", profile.name, profile.label, default);
  }
  Ok(())
}

async fn tui_main(report: &str, profile_menu: Option<ProfileMenuState>) -> Result<ExitReason> {
  panic::set_hook(Box::new(|panic_info| {
    destruct_terminal();
    better_panic::Settings::auto().create_panic_handler()(panic_info);
  }));

  let mut app = app::TaskwarriorTui::new_with_profiles(report, true, profile_menu).await?;
  let mut terminal = app.start_tui()?;
  let result = app.run(&mut terminal).await;
  app.pause_tui().await?;
  result
}

fn replacement_command(executable: &Path, profile_name: &str, report: &str, config: Option<&str>, data: Option<&str>) -> std::process::Command {
  let mut command = std::process::Command::new(executable);
  command.arg("--profile").arg(profile_name).arg("--report").arg(report);
  if let Some(config) = config {
    command.arg("--config").arg(config);
  }
  if let Some(data) = data {
    command.arg("--data").arg(data);
  }
  command.env_remove("TASKRC").env_remove("TASKDATA");
  command
}

fn restart_with_profile(profile_name: &str, report: &str, config: Option<&str>, data: Option<&str>) -> Result<()> {
  let executable = env::current_exe().context("Unable to locate the taskwarrior-tui executable")?;
  let mut command = replacement_command(&executable, profile_name, report, config, data);

  #[cfg(unix)]
  {
    use std::os::unix::process::CommandExt;
    let error = command.exec();
    Err(anyhow!(
      "Unable to switch to profile '{profile_name}' with {}: {error}",
      executable.display()
    ))
  }

  #[cfg(windows)]
  {
    command
      .spawn()
      .with_context(|| format!("Unable to switch to profile '{profile_name}' with {}", executable.display()))?;
    Ok(())
  }

  #[cfg(not(any(unix, windows)))]
  {
    let _ = command;
    bail!("Profile switching is not supported on this platform")
  }
}

fn run_main() -> Result<()> {
  better_panic::install();
  let matches = cli::generate_cli_app().get_matches();

  let config = matches.get_one::<String>("config").cloned();
  let data = matches.get_one::<String>("data").cloned();
  let taskrc = matches.get_one::<String>("taskrc").cloned();
  let taskdata = matches.get_one::<String>("taskdata").cloned();
  let requested_profile = matches.get_one::<String>("profile").cloned();
  let report = matches.get_one::<String>("report").cloned().unwrap_or_else(|| "next".to_string());

  if let Some(path) = &config {
    set_env_path_if_unset("TASKWARRIOR_TUI_CONFIG", path, "config");
  }
  if let Some(path) = &data {
    set_env_path_if_unset("TASKWARRIOR_TUI_DATA", path, "data");
  }

  let config_dir = config_directory()?;
  if matches.get_flag("list-profiles") {
    return list_profiles(&config_dir);
  }

  let direct_taskwarrior_paths = taskrc.is_some() || taskdata.is_some();
  let profile_menu = if direct_taskwarrior_paths {
    None
  } else {
    load_profile_menu(&config_dir, requested_profile.as_deref())?
  };

  if let Some(menu) = &profile_menu {
    set_profile_environment(&menu.active);
  } else {
    if let Some(path) = &taskrc {
      set_env_path_if_unset("TASKRC", path, "taskrc");
    }
    if let Some(path) = &taskdata {
      set_env_path_if_unset("TASKDATA", path, "taskdata");
    }
  }

  initialize_logging();
  debug!("report = {:?}", &report);
  debug!("config = {:?}", &config);
  debug!("profile = {:?}", profile_menu.as_ref().map(|menu| &menu.active.name));

  let exit_reason = tokio::runtime::Builder::new_multi_thread()
    .enable_all()
    .build()?
    .block_on(async { tui_main(&report, profile_menu).await })?;

  if let ExitReason::SwitchProfile(profile_name) = exit_reason {
    restart_with_profile(&profile_name, &report, config.as_deref(), data.as_deref())?;
  }
  Ok(())
}

fn main() {
  if let Err(error) = run_main() {
    eprintln!(
      "\x1b[0;31m[taskwarrior-tui error]\x1b[0m: {error:#}\n\nIf you need additional help, please report as a github issue on https://github.com/kdheepak/taskwarrior-tui"
    );
    std::process::exit(1);
  }
}

#[cfg(test)]
mod tests {
  use std::ffi::OsStr;

  use super::*;

  #[test]
  fn replacement_command_selects_profile_and_removes_inherited_task_paths() {
    let command = replacement_command(
      Path::new("/usr/bin/taskwarrior-tui"),
      "personal",
      "waiting",
      Some("/tmp/config"),
      Some("/tmp/data"),
    );
    let args: Vec<_> = command.get_args().collect();
    assert_eq!(
      args,
      [
        OsStr::new("--profile"),
        OsStr::new("personal"),
        OsStr::new("--report"),
        OsStr::new("waiting"),
        OsStr::new("--config"),
        OsStr::new("/tmp/config"),
        OsStr::new("--data"),
        OsStr::new("/tmp/data"),
      ]
    );
    let environment: Vec<_> = command.get_envs().collect();
    assert!(environment.contains(&(OsStr::new("TASKRC"), None)));
    assert!(environment.contains(&(OsStr::new("TASKDATA"), None)));
  }

  #[test]
  fn profile_conflicts_with_direct_taskwarrior_paths() {
    let taskrc = cli::generate_cli_app().try_get_matches_from(["taskwarrior-tui", "--profile", "work", "--taskrc", "x"]);
    assert!(taskrc.is_err());
    let taskdata = cli::generate_cli_app().try_get_matches_from(["taskwarrior-tui", "--profile", "work", "--taskdata", "x"]);
    assert!(taskdata.is_err());
  }
}
