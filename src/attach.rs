//! `desk attach`: placing the workspaces of a plan as sessions and tabs.
//!
//! The plan comes from another tool, which knows what the workspaces are;
//! desk only knows where they go.

use std::io::Read;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::backend::{Backend, NewTab, SessionRef, TabRef};
use crate::commands::{require_dir, shell_argv};
use crate::config::Config;
use crate::error::{AppError, Result};

const PLAN_VERSION: u64 = 1;
const STANDARD_INPUT: &str = "-";
/// The name of a workspace's own tab when it is open as a session.
const MAIN: &str = "main";

#[derive(Debug)]
pub struct Plan {
    /// The root's nearest existing ancestor. Only decides where the root goes.
    parent: Option<Workspace>,
    root: Workspace,
    /// Depth-first, so each follows its own parent.
    descendants: Vec<Workspace>,
}

#[derive(Debug, Deserialize)]
struct Workspace {
    name: String,
    symbol: Option<String>,
    /// Canonical, so it is also the key the workspace is found by.
    dir: PathBuf,
}

#[derive(Deserialize)]
struct Versioned {
    version: u64,
}

#[derive(Deserialize)]
struct RawPlan {
    parent: Option<Workspace>,
    workspaces: Vec<Workspace>,
}

impl Plan {
    /// Reads the plan from `path`, or from standard input when it is `-`.
    pub fn read(path: &Path) -> Result<Self> {
        let (origin, text) = if path == Path::new(STANDARD_INPUT) {
            let mut text = String::new();
            let read = std::io::stdin().lock().read_to_string(&mut text);
            ("standard input".to_owned(), read.map(|_| text))
        } else {
            (path.display().to_string(), std::fs::read_to_string(path))
        };
        let text = text.map_err(|source| AppError::ReadPlan { origin, source })?;
        Self::parse(&text)
    }

    pub fn parse(text: &str) -> Result<Self> {
        // The version first, so a newer plan is not reported as malformed.
        let Versioned { version } = serde_json::from_str(text).map_err(AppError::ParsePlan)?;
        if version != PLAN_VERSION {
            return Err(invalid(format!(
                "unsupported `version` {version}; expected {PLAN_VERSION}"
            )));
        }
        let raw: RawPlan = serde_json::from_str(text).map_err(AppError::ParsePlan)?;

        for workspace in raw.parent.iter().chain(&raw.workspaces) {
            workspace.validate()?;
        }
        for (index, workspace) in raw.workspaces.iter().enumerate() {
            let earlier = &raw.workspaces[..index];
            if earlier.iter().any(|other| other.name == workspace.name) {
                return Err(invalid(format!(
                    "workspace {:?} is listed twice",
                    workspace.name
                )));
            }
            if let Some(other) = earlier.iter().find(|other| other.dir == workspace.dir) {
                return Err(invalid(format!(
                    "workspaces {:?} and {:?} share the directory {}",
                    other.name,
                    workspace.name,
                    workspace.dir.display()
                )));
            }
        }
        let mut workspaces = raw.workspaces.into_iter();
        let root = workspaces
            .next()
            .ok_or_else(|| invalid("`workspaces` is empty".to_owned()))?;
        Ok(Self {
            parent: raw.parent,
            root,
            descendants: workspaces.collect(),
        })
    }
}

impl Workspace {
    fn validate(&self) -> Result<()> {
        let name = &self.name;
        if name.is_empty() {
            return Err(invalid("a workspace has an empty `name`".to_owned()));
        }
        if !self.dir.is_absolute() {
            return Err(invalid(format!(
                "`dir` of workspace {name:?} is not absolute: {}",
                self.dir.display()
            )));
        }
        match &self.symbol {
            Some(symbol) if !is_symbol(symbol) => Err(invalid(format!(
                "`symbol` of workspace {name:?} must be nonempty and contain no whitespace or \
                 control characters"
            ))),
            _ => Ok(()),
        }
    }

    fn key(&self) -> &str {
        self.dir.to_str().expect("a JSON string is UTF-8")
    }

    fn display_name(&self) -> String {
        self.labeled(&self.name)
    }

    fn main_label(&self) -> String {
        self.labeled(MAIN)
    }

    fn labeled(&self, name: &str) -> String {
        match &self.symbol {
            Some(symbol) => format!("{symbol} {name}"),
            None => name.to_owned(),
        }
    }

    fn attached(
        &self,
        session_id: String,
        status: Status,
        label: String,
        tab_id: String,
        tab_status: Status,
    ) -> Attached {
        Attached {
            name: self.name.clone(),
            session_id,
            status,
            tab: AttachedTab {
                label,
                dir: self.dir.clone(),
                id: tab_id,
                status: tab_status,
            },
        }
    }

    /// Whether a session or tab with this key and label is the workspace. A
    /// key decides when there is one; a label is all Rex has.
    fn is(&self, key: Option<&str>, label: &str) -> bool {
        match key {
            Some(key) => key == self.key(),
            None => names(label, &self.name),
        }
    }
}

fn invalid(message: String) -> AppError {
    AppError::InvalidPlan(message)
}

fn is_symbol(symbol: &str) -> bool {
    !symbol.is_empty() && !symbol.chars().any(|c| c.is_whitespace() || c.is_control())
}

/// Whether a label is `name`, with or without a symbol.
fn names(label: &str, name: &str) -> bool {
    label == name
        || label
            .split_once(' ')
            .is_some_and(|(symbol, rest)| rest == name && is_symbol(symbol))
}

#[derive(Debug, Serialize)]
pub struct Report {
    backend: &'static str,
    /// One per workspace of the plan, in its order.
    workspaces: Vec<Attached>,
    warnings: Vec<String>,
}

#[derive(Debug, Serialize)]
struct Attached {
    name: String,
    session_id: String,
    status: Status,
    tab: AttachedTab,
}

impl Attached {
    /// Whether attaching it opened or renamed anything.
    fn changed(&self) -> bool {
        self.status != Status::Reused || self.tab.status != Status::Reused
    }
}

#[derive(Debug, Serialize)]
struct AttachedTab {
    label: String,
    dir: PathBuf,
    id: String,
    status: Status,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
enum Status {
    Created,
    Reused,
    /// It was open, but a label was renamed or a key was set again.
    Reconciled,
}

pub fn attach(config: &Config, backend: &dyn Backend, plan: &Plan) -> Result<Report> {
    for workspace in std::iter::once(&plan.root).chain(&plan.descendants) {
        require_dir(&workspace.dir)?;
    }
    let argv = shell_argv(config, "");
    let attach = Attach {
        backend,
        argv: argv.as_deref(),
    };

    // Listing costs a backend call per session, so it is repeated only after
    // a workspace changed what is open.
    let mut listed = attach.list()?;
    let root = attach.one(&listed, &plan.root, Placement::Beside(plan.parent.as_ref()))?;
    let mut stale = root.changed();
    let mut workspaces = Vec::with_capacity(1 + plan.descendants.len());
    for workspace in &plan.descendants {
        if stale {
            listed = attach.list()?;
        }
        let attached = attach.one(&listed, workspace, Placement::In(&root.session_id))?;
        stale = attached.changed();
        workspaces.push(attached);
    }

    let mut warnings = Vec::new();
    backend.focus_tab(&root.session_id, &root.tab.id)?;
    // The workspace is open either way.
    if let Err(error) = backend.show_session(&root.session_id, &root.tab.id) {
        warnings.push(format!(
            "workspace {:?} is open in session {}, but it could not be shown: {error}",
            root.name, root.session_id
        ));
    }
    workspaces.insert(0, root);
    Ok(Report {
        backend: config.backend.as_str(),
        workspaces,
        warnings,
    })
}

enum Placement<'a> {
    /// Beside the parent when it is open in the session desk runs in.
    Beside(Option<&'a Workspace>),
    /// In this session, which holds the attached root.
    In(&'a str),
}

struct Listed {
    session: SessionRef,
    tabs: Vec<TabRef>,
}

/// Where a workspace is already open: a session of its own, or a tab in
/// another session.
#[derive(Clone, Copy)]
struct Open<'a> {
    listed: &'a Listed,
    tab: Option<&'a TabRef>,
}

impl Open<'_> {
    fn id(&self) -> &str {
        self.tab.map_or(&self.listed.session.id, |tab| &tab.id)
    }
}

struct Attach<'a> {
    backend: &'a dyn Backend,
    argv: Option<&'a [String]>,
}

impl Attach<'_> {
    fn one(
        &self,
        listed: &[Listed],
        workspace: &Workspace,
        placement: Placement<'_>,
    ) -> Result<Attached> {
        let mut open = find(listed, workspace);
        if open.len() > 1 {
            let candidates = join_ids(open.iter().map(Open::id));
            let mut rooted = Vec::new();
            for open in open {
                let session_id = &open.listed.session.id;
                let tabs = match open.tab {
                    Some(tab) => std::slice::from_ref(tab),
                    None => &open.listed.tabs,
                };
                if self.any_rooted(session_id, tabs, &workspace.dir)? {
                    rooted.push(open);
                }
            }
            if rooted.len() != 1 {
                return Err(AppError::AmbiguousWorkspace {
                    name: workspace.name.clone(),
                    candidates,
                });
            }
            open = rooted;
        }

        match open.first() {
            Some(Open { listed, tab: None }) => self.reuse_session(listed, workspace),
            Some(Open {
                listed,
                tab: Some(tab),
            }) => self.reuse_tab(&listed.session, tab, workspace),
            None => {
                let host = match placement {
                    Placement::In(host) => Some(host.to_owned()),
                    Placement::Beside(Some(parent)) => self
                        .backend
                        .current_session()
                        .and_then(|current| listed.iter().find(|one| one.session.id == current))
                        .filter(|here| {
                            parent.is(here.session.key.as_deref(), &here.session.label)
                                || here
                                    .tabs
                                    .iter()
                                    .any(|tab| parent.is(tab.key.as_deref(), &tab.label))
                        })
                        .map(|here| here.session.id.clone()),
                    Placement::Beside(None) => None,
                };
                let key = Some(workspace.key());
                let (session_id, label, tab_id) = match host {
                    Some(host) => {
                        let label = workspace.display_name();
                        let tab_id = self.open_tab(&host, &label, workspace)?;
                        (host, label, tab_id)
                    }
                    None => {
                        let label = workspace.main_label();
                        let tab = NewTab {
                            label: &label,
                            dir: &workspace.dir,
                            argv: self.argv,
                            key,
                        };
                        let created =
                            self.backend
                                .create_session(&workspace.display_name(), key, tab)?;
                        (created.session_id, label, created.tab_id)
                    }
                };
                let created = Status::Created;
                Ok(workspace.attached(session_id, created, label, tab_id, created))
            }
        }
    }

    /// Every session with its tabs.
    fn list(&self) -> Result<Vec<Listed>> {
        self.backend
            .sessions()?
            .into_iter()
            .map(|session| {
                Ok(Listed {
                    tabs: self.backend.tabs(&session.id)?,
                    session,
                })
            })
            .collect()
    }

    fn open_tab(&self, session_id: &str, label: &str, workspace: &Workspace) -> Result<String> {
        let tab = NewTab {
            label,
            dir: &workspace.dir,
            argv: self.argv,
            key: Some(workspace.key()),
        };
        self.backend.open_tab(session_id, tab)
    }

    /// The workspace is open as a session; its own tab is the main one.
    fn reuse_session(&self, listed: &Listed, workspace: &Workspace) -> Result<Attached> {
        let session = &listed.session;
        let main_label = workspace.main_label();
        let key = workspace.key();

        let mut mains = keyed_first(&listed.tabs, key, MAIN);
        if mains.len() > 1 {
            let candidates = join_ids(mains.iter().map(|tab| tab.id.as_str()));
            let mut rooted = Vec::new();
            for tab in mains {
                if self.any_rooted(&session.id, std::slice::from_ref(tab), &workspace.dir)? {
                    rooted.push(tab);
                }
            }
            if rooted.len() != 1 {
                return Err(AppError::AmbiguousMainTab {
                    session: session.id.clone(),
                    candidates,
                });
            }
            mains = rooted;
        }
        let (tab_id, tab_status) = match mains.first() {
            Some(tab) => (
                tab.id.clone(),
                self.reconcile_tab(session, tab, &main_label, key)?,
            ),
            None => (
                self.open_tab(&session.id, &main_label, workspace)?,
                Status::Created,
            ),
        };

        let mut changed = tab_status != Status::Reused;
        if session.key.is_none() && self.backend.set_session_key(&session.id, key)? {
            changed = true;
        }
        // Without a symbol the label may carry one the user chose.
        let display_name = workspace.display_name();
        if workspace.symbol.is_some() && session.label != display_name {
            self.backend.rename_session(&session.id, &display_name)?;
            changed = true;
        }

        let status = if changed {
            Status::Reconciled
        } else {
            Status::Reused
        };
        Ok(workspace.attached(session.id.clone(), status, main_label, tab_id, tab_status))
    }

    /// The workspace is open as a tab in another workspace's session.
    fn reuse_tab(
        &self,
        session: &SessionRef,
        tab: &TabRef,
        workspace: &Workspace,
    ) -> Result<Attached> {
        let label = workspace.display_name();
        let status = self.reconcile_tab(session, tab, &label, workspace.key())?;
        Ok(workspace.attached(session.id.clone(), status, label, tab.id.clone(), status))
    }

    /// Gives an open tab the label and key it should have.
    fn reconcile_tab(
        &self,
        session: &SessionRef,
        tab: &TabRef,
        label: &str,
        key: &str,
    ) -> Result<Status> {
        let mut status = Status::Reused;
        if tab.label != label {
            self.backend.rename_tab(&session.id, &tab.id, label)?;
            status = Status::Reconciled;
        }
        if tab.key.is_none() && self.backend.set_tab_key(&session.id, &tab.id, key)? {
            status = Status::Reconciled;
        }
        Ok(status)
    }

    /// Whether a pane in any of the tabs is working inside `dir`.
    fn any_rooted(&self, session_id: &str, tabs: &[TabRef], dir: &Path) -> Result<bool> {
        for tab in tabs {
            let cwds = self.backend.tab_cwds(session_id, &tab.id)?;
            if cwds
                .into_iter()
                .any(|cwd| cwd.canonicalize().unwrap_or(cwd).starts_with(dir))
            {
                return Ok(true);
            }
        }
        Ok(false)
    }
}

/// The sessions and tabs holding the workspace. Those carrying its key win;
/// labels are consulted only when no key matches, and only on sessions and
/// tabs without a key, since a key that differs is another workspace's.
fn find<'a>(listed: &'a [Listed], workspace: &Workspace) -> Vec<Open<'a>> {
    let by = |keyed: bool| {
        let matches = |key: &Option<String>, label: &str| {
            key.is_some() == keyed && workspace.is(key.as_deref(), label)
        };
        let mut open = Vec::new();
        for listed in listed {
            // A session of its own holds the workspace's main tab, which is
            // not a second place it is open.
            if matches(&listed.session.key, &listed.session.label) {
                open.push(Open { listed, tab: None });
                continue;
            }
            open.extend(
                listed
                    .tabs
                    .iter()
                    .filter(|tab| matches(&tab.key, &tab.label))
                    .map(|tab| Open {
                        listed,
                        tab: Some(tab),
                    }),
            );
        }
        open
    };
    let keyed = by(true);
    if keyed.is_empty() { by(false) } else { keyed }
}

/// The tabs carrying `key`, or else the tabs without a key whose label names
/// `name`.
fn keyed_first<'a>(tabs: &'a [TabRef], key: &str, name: &str) -> Vec<&'a TabRef> {
    let keyed: Vec<_> = tabs
        .iter()
        .filter(|tab| tab.key.as_deref() == Some(key))
        .collect();
    if !keyed.is_empty() {
        return keyed;
    }
    tabs.iter()
        .filter(|tab| tab.key.is_none() && names(&tab.label, name))
        .collect()
}

fn join_ids<'a>(ids: impl Iterator<Item = &'a str>) -> String {
    ids.collect::<Vec<_>>().join(", ")
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use super::*;
    use crate::config::BackendKind;
    use crate::fake::Fake;

    /// Workspaces live in directories named after them under a canonical root.
    struct Forest {
        root: PathBuf,
        _directory: tempfile::TempDir,
    }

    type Named<'a> = (&'a str, Option<&'a str>);

    impl Forest {
        fn new() -> Self {
            let directory = tempfile::tempdir().unwrap();
            Self {
                root: directory.path().canonicalize().unwrap(),
                _directory: directory,
            }
        }

        fn dir(&self, name: &str) -> PathBuf {
            let dir = self.root.join(name);
            std::fs::create_dir_all(&dir).unwrap();
            dir
        }

        fn key(&self, name: &str) -> Option<String> {
            Some(self.dir(name).to_str().unwrap().to_owned())
        }

        fn plan(&self, parent: Option<Named<'_>>, workspaces: &[Named<'_>]) -> Plan {
            let workspace = |(name, symbol): &Named<'_>| json!({"name": name, "symbol": symbol, "dir": self.dir(name), "parent": null});
            let plan = json!({
                "version": 1,
                "parent": parent.as_ref().map(workspace),
                "workspaces": workspaces.iter().map(workspace).collect::<Vec<_>>(),
            });
            Plan::parse(&plan.to_string()).unwrap()
        }
    }

    fn config() -> Config {
        Config {
            backend: BackendKind::Rex,
            shell: Some(["fish", "-C", "{command}"].map(String::from).to_vec()),
            sessions: Vec::new(),
            targets: Vec::new(),
        }
    }

    /// A fake that cannot store keys, as Rex cannot.
    fn keyless() -> Fake {
        let fake = Fake::default();
        fake.state.borrow_mut().keyless = true;
        fake
    }

    fn run(fake: &Fake, plan: &Plan) -> Value {
        serde_json::to_value(attach(&config(), fake, plan).unwrap()).unwrap()
    }

    /// Each workspace's name, status, tab label and tab status.
    fn statuses(report: &Value) -> Vec<[String; 4]> {
        let text = |value: &Value| value.as_str().unwrap().to_owned();
        report["workspaces"]
            .as_array()
            .unwrap()
            .iter()
            .map(|workspace| {
                let tab = &workspace["tab"];
                [
                    text(&workspace["name"]),
                    text(&workspace["status"]),
                    text(&tab["label"]),
                    text(&tab["status"]),
                ]
            })
            .collect()
    }

    fn layout(sessions: &[(&str, &[&str])]) -> Vec<(String, Vec<String>)> {
        sessions
            .iter()
            .map(|(label, tabs)| {
                let tabs = tabs.iter().map(|tab| (*tab).to_owned()).collect();
                ((*label).to_owned(), tabs)
            })
            .collect()
    }

    #[test]
    fn parses_the_documented_plan() {
        let plan = Plan::parse(
            r#"{
              "version": 1,
              "parent": {"name": "q4-storage", "symbol": "🌲", "dir": "/abs/path"},
              "workspaces": [
                {"name": "logical-slots", "symbol": "🚦", "dir": "/abs/slots",
                 "parent": "q4-storage"},
                {"name": "slot-tests", "symbol": null, "dir": "/abs/tests",
                 "parent": "logical-slots"}
              ]
            }"#,
        )
        .unwrap();

        assert_eq!(plan.parent.unwrap().display_name(), "🌲 q4-storage");
        assert_eq!(plan.root.display_name(), "🚦 logical-slots");
        assert_eq!(plan.root.main_label(), "🚦 main");
        assert_eq!(plan.root.key(), "/abs/slots");
        let [child] = &plan.descendants[..] else {
            panic!("expected one descendant");
        };
        assert_eq!(
            (child.display_name(), child.main_label()),
            ("slot-tests".into(), "main".into())
        );
    }

    #[test]
    fn rejects_plans_it_cannot_place_as_usage_errors() {
        let workspace = r#"{"name": "a", "symbol": null, "dir": "/a"}"#;
        let cases = [
            (
                r#"{"version": 1, "workspaces": ["#.to_owned(),
                "could not parse",
            ),
            (
                format!(r#"{{"version": 2, "workspaces": [{workspace}]}}"#),
                "unsupported `version` 2",
            ),
            (format!(r#"{{"workspaces": [{workspace}]}}"#), "version"),
            (
                r#"{"version": 1, "parent": null, "workspaces": []}"#.to_owned(),
                "`workspaces` is empty",
            ),
            (
                r#"{"version": 1, "workspaces": [{"name": "a", "dir": "a"}]}"#.to_owned(),
                "is not absolute",
            ),
            (
                r#"{"version": 1, "workspaces": [{"name": "a", "symbol": "x y", "dir": "/a"}]}"#
                    .to_owned(),
                "`symbol` of workspace \"a\"",
            ),
            (
                format!(r#"{{"version": 1, "workspaces": [{workspace}, {workspace}]}}"#),
                "listed twice",
            ),
            (
                format!(
                    r#"{{"version": 1, "workspaces": [{workspace},
                        {{"name": "b", "symbol": null, "dir": "/a"}}]}}"#
                ),
                "share the directory /a",
            ),
        ];

        for (plan, expected) in cases {
            let error = Plan::parse(&plan).unwrap_err();
            assert!(error.to_string().contains(expected), "{error}");
            assert_eq!(error.exit_code(), 2, "{error}");
        }
    }

    #[test]
    fn a_new_root_gets_a_session_with_a_main_tab_and_its_descendants_as_tabs() {
        let forest = Forest::new();
        let plan = forest.plan(None, &[("slots", Some("🚦")), ("tests", None)]);
        let fake = Fake::default();

        let report = run(&fake, &plan);

        assert_eq!(
            fake.layout(),
            layout(&[("🚦 slots", &["🚦 main", "tests"])])
        );
        assert_eq!(
            report,
            json!({
                "backend": "rex",
                "workspaces": [
                    {"name": "slots", "session_id": "id-2", "status": "created",
                     "tab": {"label": "🚦 main", "dir": forest.dir("slots"), "id": "id-1",
                             "status": "created"}},
                    {"name": "tests", "session_id": "id-2", "status": "created",
                     "tab": {"label": "tests", "dir": forest.dir("tests"), "id": "id-3",
                             "status": "created"}},
                ],
                "warnings": [],
            })
        );
        let shell = ["fish", "-C", ""].map(String::from).to_vec();
        for (label, name) in [("🚦 main", "slots"), ("tests", "tests")] {
            let (dir, argv) = fake.tab(label, |tab| (tab.dir.clone(), tab.argv.clone()));
            assert_eq!((dir, argv), (forest.dir(name), Some(shell.clone())));
        }
        assert_eq!(fake.focused(), ["🚦 main"]);
        assert_eq!(fake.state.borrow().shown, ["🚦 slots"]);
    }

    #[test]
    fn the_root_opens_beside_its_parent_only_in_the_session_desk_runs_in() {
        let forest = Forest::new();
        let parent = Some(("q4", Some("🌲")));
        let beside = forest.plan(parent, &[("slots", Some("🚦"))]);
        let orphan = forest.plan(None, &[("slots", Some("🚦"))]);
        // The sessions open beforehand, which of them desk runs in, the plan,
        // and the sessions afterwards.
        type Sessions<'a> = &'a [(&'a str, &'a [&'a str])];
        let cases: [(Sessions<'_>, Option<usize>, &Plan, Sessions<'_>); 5] = [
            (
                &[("🌲 q4", &["🌲 main"])],
                Some(0),
                &beside,
                &[("🌲 q4", &["🌲 main", "🚦 slots"])],
            ),
            (
                &[("work", &["q4"])],
                Some(0),
                &beside,
                &[("work", &["q4", "🚦 slots"])],
            ),
            (
                &[("🌲 q4", &[]), ("work", &[])],
                Some(1),
                &beside,
                &[("🌲 q4", &[]), ("work", &[]), ("🚦 slots", &["🚦 main"])],
            ),
            (
                &[("🌲 q4", &[])],
                None,
                &beside,
                &[("🌲 q4", &[]), ("🚦 slots", &["🚦 main"])],
            ),
            (
                &[("🌲 q4", &[])],
                Some(0),
                &orphan,
                &[("🌲 q4", &[]), ("🚦 slots", &["🚦 main"])],
            ),
        ];

        for (before, current, plan, after) in cases {
            let fake = keyless();
            let ids: Vec<String> = before
                .iter()
                .map(|(label, tabs)| fake.seed(label, tabs))
                .collect();
            fake.state.borrow_mut().current = current.map(|index| ids[index].clone());

            let report = run(&fake, plan);

            assert_eq!(fake.layout(), layout(after));
            assert_eq!(statuses(&report)[0][1], "created");
        }
    }

    #[test]
    fn attaching_again_reuses_everything_with_or_without_keys() {
        for fake in [Fake::default(), keyless()] {
            let forest = Forest::new();
            let plan = forest.plan(None, &[("slots", Some("🚦")), ("tests", None)]);
            let first = run(&fake, &plan);
            let listed = fake.state.borrow().listings;

            let second = run(&fake, &plan);

            assert_eq!(
                statuses(&second),
                [
                    ["slots", "reused", "🚦 main", "reused"],
                    ["tests", "reused", "tests", "reused"]
                ]
            );
            // Nothing changed, so one look at what is open serves every workspace.
            assert_eq!(fake.state.borrow().listings, listed + 1);
            for index in 0..2 {
                let (first, second) = (&first["workspaces"][index], &second["workspaces"][index]);
                assert_eq!(first["session_id"], second["session_id"]);
                assert_eq!(first["tab"]["id"], second["tab"]["id"]);
            }
            assert_eq!(
                fake.layout(),
                layout(&[("🚦 slots", &["🚦 main", "tests"])])
            );
            assert_eq!(fake.focused(), ["🚦 main", "🚦 main"]);
        }
    }

    #[test]
    fn stale_labels_are_renamed_and_reported_as_reconciled() {
        let forest = Forest::new();
        let fake = keyless();
        run(
            &fake,
            &forest.plan(None, &[("slots", Some("🚦")), ("tests", None)]),
        );

        let report = run(
            &fake,
            &forest.plan(None, &[("slots", Some("🚥")), ("tests", Some("🧪"))]),
        );

        assert_eq!(
            fake.layout(),
            layout(&[("🚥 slots", &["🚥 main", "🧪 tests"])])
        );
        assert_eq!(
            statuses(&report),
            [
                ["slots", "reconciled", "🚥 main", "reconciled"],
                ["tests", "reconciled", "🧪 tests", "reconciled"]
            ]
        );
    }

    #[test]
    fn a_session_keeps_a_symbol_the_plan_does_not_name() {
        let forest = Forest::new();
        let fake = keyless();
        fake.seed("🚦 slots", &["main"]);

        let report = run(&fake, &forest.plan(None, &[("slots", None)]));

        assert_eq!(fake.layout(), layout(&[("🚦 slots", &["main"])]));
        assert_eq!(statuses(&report), [["slots", "reused", "main", "reused"]]);
    }

    #[test]
    fn a_session_that_lost_its_main_tab_gets_a_new_one() {
        let forest = Forest::new();
        let fake = keyless();
        fake.seed("slots", &["scratch"]);

        let report = run(&fake, &forest.plan(None, &[("slots", None)]));

        assert_eq!(fake.layout(), layout(&[("slots", &["scratch", "main"])]));
        assert_eq!(
            statuses(&report),
            [["slots", "reconciled", "main", "created"]]
        );
        assert_eq!(fake.focused(), ["main"]);
    }

    #[test]
    fn a_cwd_inside_the_workspace_settles_which_of_several_labels_is_it() {
        let forest = Forest::new();
        let plan = forest.plan(None, &[("slots", Some("🚦"))]);
        let fake = keyless();
        fake.seed("a", &["slots"]);
        let b = fake.seed("b", &["🚦 slots"]);
        let cwds = |session: usize, cwds: &[PathBuf]| {
            fake.state.borrow_mut().sessions[session].tabs[0].cwds = cwds.to_vec();
        };
        let untouched = layout(&[("a", &["slots"]), ("b", &["🚦 slots"])]);

        let nowhere = attach(&config(), &fake, &plan).unwrap_err();
        cwds(1, &[PathBuf::from("/"), forest.dir("slots/src")]);
        let settled = run(&fake, &plan);
        cwds(0, &[forest.dir("slots")]);
        let both = attach(&config(), &fake, &plan).unwrap_err();

        for error in [nowhere, both] {
            assert_eq!(
                error.to_string(),
                "multiple sessions or tabs match workspace \"slots\": id-1, id-3"
            );
            assert_eq!(error.exit_code(), 1);
        }
        assert_eq!(settled["workspaces"][0]["session_id"], json!(b));
        assert_eq!(settled["workspaces"][0]["tab"]["id"], "id-3");
        assert_eq!(
            statuses(&settled),
            [["slots", "reused", "🚦 slots", "reused"]]
        );
        assert_eq!(fake.layout(), untouched);
    }

    #[test]
    fn a_cwd_settles_which_of_several_main_tabs_is_the_workspaces() {
        let forest = Forest::new();
        let plan = forest.plan(None, &[("slots", Some("🚦"))]);
        let fake = keyless();
        fake.seed("slots", &["main", "🚦 main"]);

        let ambiguous = attach(&config(), &fake, &plan).unwrap_err().to_string();
        fake.state.borrow_mut().sessions[0].tabs[0].cwds = vec![forest.dir("slots")];
        let report = run(&fake, &plan);

        assert!(ambiguous.contains("id-1, id-2"), "{ambiguous}");
        assert_eq!(report["workspaces"][0]["tab"]["id"], "id-1");
        assert_eq!(
            fake.layout(),
            layout(&[("🚦 slots", &["🚦 main", "🚦 main"])])
        );
    }

    #[test]
    fn a_key_finds_a_relabeled_workspace_before_any_label_does() {
        let forest = Forest::new();
        let plan = forest.plan(None, &[("slots", None), ("tests", None)]);
        let fake = Fake::default();
        let first = run(&fake, &plan);
        {
            let mut state = fake.state.borrow_mut();
            state.sessions[0].label = "mine".into();
            state.sessions[0].tabs[1].label = "renamed".into();
        }
        fake.seed("decoy", &["tests"]);

        let second = run(&fake, &plan);

        assert_eq!(
            fake.layout(),
            layout(&[("mine", &["main", "tests"]), ("decoy", &["tests"])])
        );
        assert_eq!(
            statuses(&second),
            [
                ["slots", "reused", "main", "reused"],
                ["tests", "reconciled", "tests", "reconciled"]
            ]
        );
        assert_eq!(
            first["workspaces"][1]["tab"]["id"],
            second["workspaces"][1]["tab"]["id"]
        );
    }

    #[test]
    fn a_label_under_another_key_is_another_workspace() {
        let forest = Forest::new();
        let plan = forest.plan(None, &[("slots", None)]);
        let fake = Fake::default();
        fake.seed("slots", &["main"]);
        fake.state.borrow_mut().sessions[0].key = Some("/elsewhere/slots".into());

        let report = run(&fake, &plan);

        assert_eq!(
            fake.layout(),
            layout(&[("slots", &["main"]), ("slots", &["main"])])
        );
        assert_eq!(statuses(&report), [["slots", "created", "main", "created"]]);
    }

    #[test]
    fn a_workspace_found_by_label_is_given_its_key_once() {
        let forest = Forest::new();
        let plan = forest.plan(None, &[("slots", None), ("tests", None)]);
        let fake = Fake::default();
        fake.seed("slots", &["main", "tests"]);

        let first = run(&fake, &plan);
        let second = run(&fake, &plan);

        assert_eq!(
            statuses(&first),
            [
                ["slots", "reconciled", "main", "reconciled"],
                ["tests", "reconciled", "tests", "reconciled"]
            ]
        );
        assert_eq!(statuses(&second)[0][1], "reused");
        assert_eq!(statuses(&second)[1][1], "reused");
        let state = fake.state.borrow();
        let session = &state.sessions[0];
        assert_eq!(session.key, forest.key("slots"));
        assert_eq!(session.tabs[0].key, forest.key("slots"));
        assert_eq!(session.tabs[1].key, forest.key("tests"));
    }

    #[test]
    fn a_session_that_cannot_be_shown_is_a_warning_not_a_failure() {
        let forest = Forest::new();
        let fake = Fake::default();
        fake.state.borrow_mut().headless = true;

        let report = run(&fake, &forest.plan(None, &[("slots", None)]));

        let [warning] = &report["warnings"].as_array().unwrap()[..] else {
            panic!("expected one warning: {report}");
        };
        let warning = warning.as_str().unwrap();
        assert!(
            warning.contains("workspace \"slots\" is open in session id-2"),
            "{warning}"
        );
        assert_eq!(statuses(&report), [["slots", "created", "main", "created"]]);
        assert_eq!(fake.focused(), ["main"]);
    }

    #[test]
    fn a_missing_directory_anywhere_in_the_plan_opens_nothing() {
        let forest = Forest::new();
        let plan = forest.plan(None, &[("slots", None), ("tests", None)]);
        std::fs::remove_dir(forest.root.join("tests")).unwrap();
        let fake = Fake::default();

        let error = attach(&config(), &fake, &plan).unwrap_err();

        assert!(matches!(error, AppError::MissingDirectory(_)), "{error}");
        assert!(fake.layout().is_empty());
    }
}
