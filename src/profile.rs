use std::{
  collections::BTreeMap,
  fs,
  path::{Path, PathBuf},
};

use anyhow::{Context, Result, anyhow, bail};
use path_clean::PathClean;
use ratatui::style::Color;
use serde::{Deserialize, Serialize};

use crate::table::TaskwarriorTuiTableState;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ProfileColor {
  Black,
  Red,
  Green,
  Yellow,
  Blue,
  Magenta,
  Cyan,
  Gray,
  DarkGray,
  LightRed,
  LightGreen,
  LightYellow,
  LightBlue,
  LightMagenta,
  LightCyan,
  White,
}

impl ProfileColor {
  pub fn as_ratatui_color(self) -> Color {
    match self {
      Self::Black => Color::Black,
      Self::Red => Color::Red,
      Self::Green => Color::Green,
      Self::Yellow => Color::Yellow,
      Self::Blue => Color::Blue,
      Self::Magenta => Color::Magenta,
      Self::Cyan => Color::Cyan,
      Self::Gray => Color::Gray,
      Self::DarkGray => Color::DarkGray,
      Self::LightRed => Color::LightRed,
      Self::LightGreen => Color::LightGreen,
      Self::LightYellow => Color::LightYellow,
      Self::LightBlue => Color::LightBlue,
      Self::LightMagenta => Color::LightMagenta,
      Self::LightCyan => Color::LightCyan,
      Self::White => Color::White,
    }
  }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Profile {
  pub taskrc: PathBuf,
  pub taskdata: PathBuf,
  pub label: Option<String>,
  pub color: Option<ProfileColor>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProfilesConfig {
  pub default: Option<String>,
  pub profiles: BTreeMap<String, Profile>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ActiveProfile {
  pub name: String,
  pub taskrc: PathBuf,
  pub taskdata: PathBuf,
  pub label: String,
  pub color: Option<ProfileColor>,
}

impl ProfilesConfig {
  pub fn load(path: &Path) -> Result<Self> {
    let contents = fs::read_to_string(path).with_context(|| format!("Unable to read profile configuration at {}", path.display()))?;
    let mut config: Self = toml::from_str(&contents).with_context(|| format!("Invalid profile configuration at {}", path.display()))?;
    let base_dir = path
      .parent()
      .ok_or_else(|| anyhow!("Unable to resolve the profile configuration directory"))?;
    let home_dir = dirs::home_dir().ok_or_else(|| anyhow!("Unable to resolve the home directory for profile paths"))?;
    config.validate_and_resolve(base_dir, &home_dir)?;
    Ok(config)
  }

  fn validate_and_resolve(&mut self, base_dir: &Path, home_dir: &Path) -> Result<()> {
    if self.profiles.is_empty() {
      bail!("Profile configuration must define at least one profile");
    }

    for (name, profile) in &mut self.profiles {
      validate_profile_name(name)?;
      if let Some(label) = &profile.label {
        if label.trim().is_empty() {
          bail!("Profile '{name}' has an empty 'label' field");
        }
        if label.chars().any(char::is_control) {
          bail!("Profile '{name}' has control characters in its 'label' field");
        }
      }

      profile.taskrc =
        resolve_path(&profile.taskrc, base_dir, home_dir).with_context(|| format!("Profile '{name}' has an invalid 'taskrc' field"))?;
      profile.taskdata =
        resolve_path(&profile.taskdata, base_dir, home_dir).with_context(|| format!("Profile '{name}' has an invalid 'taskdata' field"))?;

      if !profile.taskrc.is_file() {
        bail!("Profile '{name}' has a 'taskrc' path that is not a file: {}", profile.taskrc.display());
      }
      fs::File::open(&profile.taskrc).with_context(|| format!("Profile '{name}' has an unreadable 'taskrc' file at {}", profile.taskrc.display()))?;

      if profile.taskdata.exists() {
        if !profile.taskdata.is_dir() {
          bail!(
            "Profile '{name}' has a 'taskdata' path that is not a directory: {}",
            profile.taskdata.display()
          );
        }
      } else {
        let parent = profile
          .taskdata
          .parent()
          .ok_or_else(|| anyhow!("Profile '{name}' has an invalid 'taskdata' field"))?;
        fs::create_dir_all(parent).with_context(|| format!("Profile '{name}' has an uncreatable 'taskdata' parent at {}", parent.display()))?;
      }
    }

    if let Some(default) = &self.default
      && !self.profiles.contains_key(default)
    {
      bail!("Default profile '{default}' is not defined in 'profiles'");
    }

    Ok(())
  }

  pub fn resolve(&self, name: &str) -> Result<ActiveProfile> {
    let profile = self.profiles.get(name).ok_or_else(|| anyhow!("Unknown profile '{name}'"))?;
    Ok(ActiveProfile {
      name: name.to_string(),
      taskrc: profile.taskrc.clone(),
      taskdata: profile.taskdata.clone(),
      label: profile.label.clone().unwrap_or_else(|| name.to_string()),
      color: profile.color,
    })
  }

  pub fn resolved_profiles(&self) -> Result<Vec<ActiveProfile>> {
    self.profiles.keys().map(|name| self.resolve(name)).collect()
  }
}

fn validate_profile_name(name: &str) -> Result<()> {
  let mut characters = name.chars();
  let valid = characters.next().is_some_and(|character| character.is_ascii_alphanumeric())
    && characters.all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.'));
  if valid {
    Ok(())
  } else {
    bail!("Invalid profile name '{name}'; start with a letter or number and use only letters, numbers, '.', '-', and '_'")
  }
}

fn resolve_path(path: &Path, base_dir: &Path, home_dir: &Path) -> Result<PathBuf> {
  let value = path.to_string_lossy();
  let expanded = if value == "~" {
    home_dir.to_path_buf()
  } else if let Some(remainder) = value.strip_prefix("~/").or_else(|| value.strip_prefix("~\\")) {
    home_dir.join(remainder)
  } else if value.starts_with('~') {
    bail!("Only '~' and '~/' home-relative paths are supported")
  } else {
    path.to_path_buf()
  };

  let absolute = if expanded.is_absolute() { expanded } else { base_dir.join(expanded) };
  Ok(absolute.clean())
}

pub struct ProfileMenuState {
  pub active: ActiveProfile,
  pub profiles: Vec<ActiveProfile>,
  pub search: String,
  pub table_state: TaskwarriorTuiTableState,
}

impl ProfileMenuState {
  pub fn new(active: ActiveProfile, profiles: Vec<ActiveProfile>) -> Self {
    Self {
      active,
      profiles,
      search: String::new(),
      table_state: TaskwarriorTuiTableState::default(),
    }
  }

  pub fn filtered_indices(&self) -> Vec<usize> {
    let query = self.search.to_lowercase();
    self
      .profiles
      .iter()
      .enumerate()
      .filter(|(_, profile)| query.is_empty() || profile.name.to_lowercase().contains(&query) || profile.label.to_lowercase().contains(&query))
      .map(|(index, _)| index)
      .collect()
  }

  pub fn selected_profile(&self) -> Option<&ActiveProfile> {
    let filtered_index = self.table_state.current_selection()?;
    let profile_index = *self.filtered_indices().get(filtered_index)?;
    self.profiles.get(profile_index)
  }

  pub fn select_active(&mut self) {
    let position = self
      .filtered_indices()
      .iter()
      .position(|&index| self.profiles[index].name == self.active.name)
      .unwrap_or(0);
    self.table_state.select(Some(position));
  }

  pub fn next(&mut self) {
    let len = self.filtered_indices().len();
    if len == 0 {
      self.table_state.select(None);
      return;
    }
    let current = self.table_state.current_selection().unwrap_or(0);
    self.table_state.select(Some((current + 1) % len));
  }

  pub fn previous(&mut self) {
    let len = self.filtered_indices().len();
    if len == 0 {
      self.table_state.select(None);
      return;
    }
    let current = self.table_state.current_selection().unwrap_or(0);
    self.table_state.select(Some(current.checked_sub(1).unwrap_or(len - 1)));
  }
}

#[cfg(test)]
mod tests {
  use std::{
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
  };

  use super::*;

  static NEXT_TEST_DIRECTORY: AtomicU64 = AtomicU64::new(0);

  struct TestDirectory(PathBuf);

  impl TestDirectory {
    fn new() -> Self {
      let timestamp = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
      let sequence = NEXT_TEST_DIRECTORY.fetch_add(1, Ordering::Relaxed);
      let path = std::env::temp_dir().join(format!("taskwarrior-tui-profile-test-{}-{timestamp}-{sequence}", std::process::id()));
      fs::create_dir_all(&path).unwrap();
      Self(path)
    }
  }

  impl Drop for TestDirectory {
    fn drop(&mut self) {
      let _ = fs::remove_dir_all(&self.0);
    }
  }

  fn write_config(directory: &Path, contents: &str) -> PathBuf {
    fs::write(directory.join("work.taskrc"), "data.location=unused\n").unwrap();
    let path = directory.join("profiles.toml");
    fs::write(&path, contents).unwrap();
    path
  }

  #[test]
  fn loads_and_resolves_valid_profiles() {
    let directory = TestDirectory::new();
    let path = write_config(
      &directory.0,
      r#"
default = "work"

[profiles.work]
taskrc = "work.taskrc"
taskdata = "data/work"
label = "Work"
color = "yellow"
"#,
    );

    let config = ProfilesConfig::load(&path).unwrap();
    let active = config.resolve("work").unwrap();
    assert_eq!(active.label, "Work");
    assert_eq!(active.color, Some(ProfileColor::Yellow));
    assert_eq!(active.taskrc, directory.0.join("work.taskrc"));
    assert_eq!(active.taskdata, directory.0.join("data/work"));
    assert!(directory.0.join("data").is_dir());
  }

  #[test]
  fn rejects_unknown_default_profile() {
    let directory = TestDirectory::new();
    let path = write_config(
      &directory.0,
      r#"
default = "personal"
[profiles.work]
taskrc = "work.taskrc"
taskdata = "work"
"#,
    );

    assert!(ProfilesConfig::load(&path).unwrap_err().to_string().contains("personal"));
  }

  #[test]
  fn rejects_missing_required_fields_and_unknown_colors() {
    let directory = TestDirectory::new();
    let missing = write_config(&directory.0, "[profiles.work]\ntaskrc = \"work.taskrc\"\n");
    assert!(format!("{:#}", ProfilesConfig::load(&missing).unwrap_err()).contains("taskdata"));

    let invalid_color = write_config(
      &directory.0,
      "[profiles.work]\ntaskrc = \"work.taskrc\"\ntaskdata = \"work\"\ncolor = \"orange\"\n",
    );
    assert!(format!("{:#}", ProfilesConfig::load(&invalid_color).unwrap_err()).contains("color"));
  }

  #[test]
  fn rejects_invalid_names_labels_and_taskrc_paths() {
    let directory = TestDirectory::new();
    let invalid_name = write_config(&directory.0, "[profiles.'.work']\ntaskrc = \"work.taskrc\"\ntaskdata = \"work\"\n");
    assert!(format!("{:#}", ProfilesConfig::load(&invalid_name).unwrap_err()).contains("Invalid profile name"));

    let invalid_label = write_config(
      &directory.0,
      "[profiles.work]\ntaskrc = \"work.taskrc\"\ntaskdata = \"work\"\nlabel = \"Work\\nTasks\"\n",
    );
    assert!(format!("{:#}", ProfilesConfig::load(&invalid_label).unwrap_err()).contains("control characters"));

    fs::remove_file(directory.0.join("work.taskrc")).unwrap();
    fs::create_dir(directory.0.join("work.taskrc")).unwrap();
    let invalid_taskrc = directory.0.join("profiles.toml");
    fs::write(&invalid_taskrc, "[profiles.work]\ntaskrc = \"work.taskrc\"\ntaskdata = \"work\"\n").unwrap();
    assert!(format!("{:#}", ProfilesConfig::load(&invalid_taskrc).unwrap_err()).contains("not a file"));
  }

  #[test]
  fn rejects_duplicate_profiles_and_malformed_toml() {
    let directory = TestDirectory::new();
    let duplicate = write_config(
      &directory.0,
      "[profiles.work]\ntaskrc = \"work.taskrc\"\ntaskdata = \"work\"\n[profiles.work]\ntaskrc = \"work.taskrc\"\ntaskdata = \"work\"\n",
    );
    assert!(ProfilesConfig::load(&duplicate).is_err());

    let malformed = write_config(&directory.0, "default = [\n");
    assert!(ProfilesConfig::load(&malformed).is_err());
  }

  #[test]
  fn expands_home_and_cleans_relative_paths() {
    let base = Path::new("/tmp/config/taskwarrior-tui");
    let home = Path::new("/home/example");
    assert_eq!(
      resolve_path(Path::new("~/task/work"), base, home).unwrap(),
      PathBuf::from("/home/example/task/work")
    );
    assert_eq!(
      resolve_path(Path::new("../task/./work"), base, home).unwrap(),
      PathBuf::from("/tmp/config/task/work")
    );
    assert!(resolve_path(Path::new("~someone/task"), base, home).is_err());
  }

  #[test]
  fn profile_menu_filters_by_name_and_label() {
    let profile = |name: &str, label: &str| ActiveProfile {
      name: name.to_string(),
      taskrc: PathBuf::new(),
      taskdata: PathBuf::new(),
      label: label.to_string(),
      color: None,
    };
    let active = profile("work", "Office");
    let mut menu = ProfileMenuState::new(active.clone(), vec![profile("personal", "Home"), active]);
    menu.search = "off".to_string();
    assert_eq!(menu.filtered_indices(), vec![1]);
    assert_eq!(menu.selected_profile().unwrap().name, "work");
  }
}
