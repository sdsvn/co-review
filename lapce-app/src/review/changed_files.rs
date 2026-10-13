//! The change in the file explorer: a window on a change review's code folder colours the files the change adds or
//! modifies, and the folders holding them, with their lines added and removed. **Changed files only** narrows the tree
//! to them (on by default for a pull request's worktree, off for your own change; the choice is remembered per review),
//! and a changed file opens as the change: base ↔ head side by side, or, for a new file, its lines marked as added.

use std::{
    collections::{BTreeMap, HashMap},
    path::{Path, PathBuf},
    rc::Rc,
    sync::Arc,
};

use floem::{
    View,
    ext_event::create_ext_action,
    reactive::{RwSignal, Scope, SignalGet, SignalUpdate, SignalWith},
    style::CursorStyle,
    views::{Decorators, VirtualVector, container, empty, label, stack},
};
use lapce_core::directory::Directory;
use lapce_rpc::file::{FileNodeItem, FileNodeViewData, FileNodeViewKind, Naming};
use review_bundle::patch::{PatchFile, parse_patch};
use review_model::Review;

use super::code;
use crate::{
    config::{LapceConfig, color::LapceColor},
    file_explorer::{data::FileExplorerData, node::FileNodeVirtualList},
    window_tab::WindowTabData,
};

/// One file of a change: its tag as the patch gives it (`modified`, `new file`, `deleted`, `renamed`, `binary`) and
/// its lines added and removed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChangedFile {
    pub tag: String,
    pub added: u32,
    pub removed: u32,
    /// The first line the change adds (else the first it shows) on the new side.
    pub first_line: Option<u32>,
}

/// A change review's files, read from the patches of its review directory.
#[derive(Debug)]
pub struct Change {
    pub review: Arc<Review>,
    /// By repository-relative path.
    pub files: BTreeMap<String, ChangedFile>,
    /// Lines added and removed under each folder holding a changed file (repository-relative; `""` is the root).
    folders: HashMap<String, (u32, u32)>,
}

impl Change {
    fn new(review: Arc<Review>, patches: &[PatchFile]) -> Self {
        let mut files = BTreeMap::new();
        let mut folders: HashMap<String, (u32, u32)> = HashMap::new();
        for file in patches {
            if file.path.starts_with("..") || file.path.starts_with('/') {
                continue;
            }
            let entry = files.entry(file.path.clone()).or_insert(ChangedFile {
                tag: file.tag.clone(),
                added: 0,
                removed: 0,
                first_line: file
                    .rows
                    .iter()
                    .find(|row| row.t == "add")
                    .or_else(|| file.rows.iter().find(|row| row.n.is_some()))
                    .and_then(|row| row.n),
            });
            entry.added += file.added;
            entry.removed += file.removed;
            if file.tag == "deleted" {
                continue;
            }
            let mut folder = file.path.as_str();
            loop {
                folder = folder.rfind('/').map_or("", |index| &folder[..index]);
                let counts = folders.entry(folder.to_string()).or_default();
                counts.0 += file.added;
                counts.1 += file.removed;
                if folder.is_empty() {
                    break;
                }
            }
        }
        Self {
            review,
            files,
            folders,
        }
    }

    /// A changed file on disk (not deleted), by repository-relative path.
    pub fn file(&self, path: &str) -> Option<&ChangedFile> {
        self.files.get(path).filter(|file| file.tag != "deleted")
    }

    /// Whether a repository-relative path is a changed file on disk, or a folder holding one.
    fn touches(&self, path: &str) -> bool {
        self.file(path).is_some() || self.folders.contains_key(path)
    }

    /// Lines added and removed in a changed file or under a folder.
    fn counts(&self, path: &str, is_dir: bool) -> Option<(u32, u32)> {
        if is_dir {
            self.folders.get(path).copied()
        } else {
            self.file(path).map(|file| (file.added, file.removed))
        }
    }
}

/// The change of a window, if it is on a change review's code folder: the active review when it is one, else the most
/// recently updated.
#[derive(Clone)]
pub struct ChangedFiles {
    /// The window's folder (resolved through symbolic links).
    root: Option<PathBuf>,
    /// The change review whose code folder is this window's, with its files.
    pub change: RwSignal<Option<Rc<Change>>>,
    /// Whether the explorer shows only the changed files and their folders.
    pub changed_only: RwSignal<bool>,
}

impl ChangedFiles {
    pub fn new(
        cx: Scope,
        root: Option<PathBuf>,
        reviews: RwSignal<im::Vector<Arc<Review>>>,
        active: RwSignal<Option<String>>,
    ) -> Self {
        let data = Self {
            root: root.clone(),
            change: cx.create_rw_signal(None),
            changed_only: cx.create_rw_signal(false),
        };
        let Some(root) = root else {
            return data;
        };
        let (change, changed_only) = (data.change, data.changed_only);
        cx.create_effect(move |loaded_for: Option<Option<String>>| {
            // The active review when it is a change on this folder, else the latest such change.
            let active = active.get();
            let review = reviews.with(|reviews| {
                let changes = || {
                    reviews.iter().filter(|review| {
                        review.archived_at.is_none()
                            && code::code_folder(review).is_some_and(|code| {
                                review_store::real_path(&code) == root
                            })
                    })
                };
                changes()
                    .find(|review| Some(&review.id) == active.as_ref())
                    .or_else(|| changes().next())
                    .cloned()
            });
            // Read again only for a new change: another review, or a new head or base.
            let key = review.as_ref().and_then(|review| {
                let bundle = review.bundle.as_ref()?;
                Some(format!(
                    "{}\n{}\n{}\n{}",
                    review.id,
                    bundle.dir,
                    bundle.head.as_deref().unwrap_or_default(),
                    bundle.base.as_deref().unwrap_or_default()
                ))
            });
            if loaded_for.as_ref() == Some(&key) {
                return key;
            }
            let Some(review) = review.filter(|_| key.is_some()) else {
                change.set(None);
                changed_only.set(false);
                return key;
            };
            let send = create_ext_action(cx, move |(loaded, choice): (Change, Option<bool>)| {
                let default = loaded
                    .review
                    .bundle
                    .as_ref()
                    .and_then(|bundle| bundle.worktree)
                    .unwrap_or(false);
                let has_files = !loaded.files.is_empty();
                let only = has_files && choice.unwrap_or(default);
                tracing::info!(
                    "the explorer shows the change of review {}: {} files, changed files only: {only}",
                    loaded.review.id,
                    loaded.files.len(),
                );
                change.set(has_files.then(|| Rc::new(loaded)));
                changed_only.set(only);
            });
            let loading = std::thread::Builder::new()
                .name("ReviewChangedFiles".to_owned())
                .spawn(move || {
                    let files = read_patches(&review);
                    let choice = read_choices().get(&review.id).copied();
                    send((Change::new(review, &files), choice));
                });
            if let Err(error) = loading {
                tracing::error!("could not read a change's files: {error}");
            }
            key
        });
        data
    }

    /// Shows only the changed files, or all files, and remembers the choice for the review.
    pub fn set_changed_only(&self, on: bool) {
        self.changed_only.set(on);
        let Some(review_id) = self
            .change
            .with_untracked(|change| change.as_ref().map(|change| change.review.id.clone()))
        else {
            return;
        };
        let saved = std::thread::Builder::new()
            .name("ReviewExplorerChoice".to_owned())
            .spawn(move || {
                let mut choices = read_choices();
                choices.insert(review_id, on);
                write_choices(&choices);
            });
        if let Err(error) = saved {
            tracing::error!("could not remember the explorer's choice: {error}");
        }
    }

    /// The change's file at an absolute path of this window.
    pub fn file_at(&self, path: &Path) -> Option<(Rc<Change>, String)> {
        let relative = relative(self.root.as_deref()?, path)?;
        self.change.with_untracked(|change| {
            let change = change.as_ref()?;
            change.file(&relative)?;
            Some((Rc::clone(change), relative))
        })
    }

    /// The explorer's rows: Lapce's tree, or with **Changed files only** the part of it holding the change.
    pub fn explorer_list(&self, root: FileNodeItem, naming: Naming) -> ExplorerList {
        let narrowed = self.changed_only.get().then(|| {
            self.change.with(|change| change.as_ref().map(|change| changed_rows(&root, change)))
        });
        match narrowed.flatten() {
            Some(rows) => ExplorerList::Changed(rows),
            None => ExplorerList::All(FileNodeVirtualList::new(root, naming)),
        }
    }

    /// The theme colour of a file or folder the change touches.
    pub fn color(&self, path: &Path, is_dir: bool) -> Option<&'static str> {
        let relative = relative(self.root.as_deref()?, path)?;
        self.change.with(|change| {
            let change = change.as_ref()?;
            if is_dir {
                return change
                    .folders
                    .contains_key(&relative)
                    .then_some(LapceColor::SOURCE_CONTROL_MODIFIED);
            }
            Some(match change.file(&relative)?.tag.as_str() {
                "new file" => LapceColor::SOURCE_CONTROL_ADDED,
                _ => LapceColor::SOURCE_CONTROL_MODIFIED,
            })
        })
    }
}

/// What the explorer's virtual list shows.
pub enum ExplorerList {
    All(FileNodeVirtualList),
    Changed(Vec<FileNodeViewData>),
}

impl VirtualVector<FileNodeViewData> for ExplorerList {
    fn total_len(&self) -> usize {
        match self {
            ExplorerList::All(list) => list.total_len(),
            ExplorerList::Changed(rows) => rows.len(),
        }
    }

    fn slice(
        &mut self,
        range: std::ops::Range<usize>,
    ) -> impl Iterator<Item = FileNodeViewData> {
        let rows: Vec<FileNodeViewData> = match self {
            ExplorerList::All(list) => list.slice(range).collect(),
            ExplorerList::Changed(rows) => {
                let end = range.end.min(rows.len());
                rows[range.start.min(end)..end].to_vec()
            }
        };
        rows.into_iter()
    }
}

/// The open part of the tree that holds the change: the root, the folders holding changed files and those files.
fn changed_rows(root: &FileNodeItem, change: &Change) -> Vec<FileNodeViewData> {
    fn walk(
        node: &FileNodeItem,
        root: &Path,
        change: &Change,
        level: usize,
        rows: &mut Vec<FileNodeViewData>,
    ) {
        for child in node.sorted_children() {
            let Some(path) = relative(root, &child.path) else {
                continue;
            };
            if !change.touches(&path) {
                continue;
            }
            rows.push(FileNodeViewData {
                kind: FileNodeViewKind::Path(child.path.clone()),
                is_dir: child.is_dir,
                is_root: false,
                open: child.open,
                level,
            });
            if child.is_dir && child.open {
                walk(child, root, change, level + 1, rows);
            }
        }
    }
    let mut rows = vec![FileNodeViewData {
        kind: FileNodeViewKind::Path(root.path.clone()),
        is_dir: true,
        is_root: true,
        open: root.open,
        level: 1,
    }];
    if root.open {
        walk(root, &root.path, change, 2, &mut rows);
    }
    rows
}

/// A path relative to `root` with `/` separators (`""` for the root itself).
fn relative(root: &Path, path: &Path) -> Option<String> {
    let relative = path.strip_prefix(root).ok()?;
    Some(
        relative
            .components()
            .map(|component| component.as_os_str().to_string_lossy())
            .collect::<Vec<_>>()
            .join("/"),
    )
}

/// The explorer section: the **Changed files only** / **All files** switch above the tree while the window shows a
/// change, and the tree opened down to every changed file whenever it is narrowed.
pub fn explorer_section(
    changed: ChangedFiles,
    explorer: FileExplorerData,
    tree: impl View + 'static,
) -> impl View {
    let config = explorer.common.config;
    {
        let changed = changed.clone();
        let explorer = explorer.clone();
        explorer.common.scope.create_effect(move |_| {
            if !changed.changed_only.get() {
                return;
            }
            let files: Vec<PathBuf> = changed.change.with(|change| {
                change
                    .as_ref()
                    .map(|change| {
                        let root = changed.root.clone().unwrap_or_default();
                        change
                            .files
                            .iter()
                            .filter(|(_, file)| file.tag != "deleted")
                            .map(|(path, _)| root.join(path))
                            .collect()
                    })
                    .unwrap_or_default()
            });
            for file in files {
                open_folders_of(&explorer, file);
            }
        });
    }
    let shown = changed.change;
    let changed_only = changed.changed_only;
    let switch = label(move || if changed_only.get() { "Changed files only" } else { "All files" })
    .on_click_stop(move |_| {
        changed.set_changed_only(!changed.changed_only.get_untracked());
    })
    .style(move |s| {
        let config = config.get();
        s.padding_horiz(10.0)
            .padding_vert(3.0)
            .border_radius(4.0)
            .cursor(CursorStyle::Pointer)
            .color(config.color(LapceColor::PANEL_FOREGROUND))
            .background(config.color(LapceColor::PANEL_CURRENT_BACKGROUND))
            .hover(|s| s.background(config.color(LapceColor::PANEL_HOVERED_BACKGROUND)))
    });
    let counts = label(move || {
        shown.with(|change| {
            change
                .as_ref()
                .map(|change| {
                    let (added, removed) = change.folders.get("").copied().unwrap_or_default();
                    let files = change.files.len();
                    format!(
                        "{files} file{} +{added} −{removed}",
                        if files == 1 { "" } else { "s" }
                    )
                })
                .unwrap_or_default()
        })
    })
    .style(move |s| {
        s.margin_left(8.0)
            .color(config.get().color(LapceColor::PANEL_FOREGROUND_DIM))
    });
    stack((
        stack((switch, counts, empty().style(|s| s.flex_grow(1.0_f32))))
            .style(move |s| {
                s.width_full()
                    .items_center()
                    .padding_horiz(6.0)
                    .padding_vert(4.0)
                    .apply_if(shown.with(Option::is_none), |s| s.hide())
            }),
        container(tree).style(|s| s.width_full().flex_grow(1.0_f32).flex_basis(0.0_f32)),
    ))
    .style(|s| s.flex_col().size_full())
}

/// Opens (reading as needed) the folders holding `path` in the explorer's tree, without selecting or scrolling.
fn open_folders_of(explorer: &FileExplorerData, path: PathBuf) {
    let unread = explorer
        .root
        .try_update(|root| {
            let top = root.path.clone();
            let mut folders: Vec<PathBuf> = path
                .ancestors()
                .skip(1)
                .take_while(|folder| folder.starts_with(&top))
                .map(Path::to_path_buf)
                .collect();
            folders.reverse();
            let mut unread = None;
            let mut deepest = None;
            for folder in folders {
                let Some(node) = root.get_file_node_mut(&folder) else {
                    break;
                };
                if !node.is_dir {
                    break;
                }
                let changed = !node.open;
                node.open = true;
                if changed {
                    deepest = Some(folder.clone());
                }
                if !node.read {
                    unread = Some(folder);
                    break;
                }
            }
            if let Some(deepest) = deepest {
                root.update_node_count_recursive(&deepest);
            }
            unread
        })
        .flatten();
    if let Some(folder) = unread {
        let reopened = explorer.clone();
        explorer.read_dir_cb(&folder, move |read| {
            if read {
                open_folders_of(&reopened, path);
            }
        });
    }
}

/// `+12 −3` after a changed file's or folder's name.
pub fn explorer_decoration(
    changed: ChangedFiles,
    path: Option<PathBuf>,
    is_dir: bool,
    config: floem::reactive::ReadSignal<Arc<LapceConfig>>,
) -> impl View {
    let counts = Rc::new(move || {
        let root = changed.root.as_deref()?;
        let relative = relative(root, path.as_deref()?)?;
        changed
            .change
            .with(|change| change.as_ref()?.counts(&relative, is_dir))
    });
    let (added_counts, removed_counts) = (Rc::clone(&counts), Rc::clone(&counts));
    stack((
        label(move || added_counts().map(|(added, _)| format!("+{added}")).unwrap_or_default()).style(
            move |s| {
                s.margin_left(6.0)
                    .color(config.get().color(LapceColor::SOURCE_CONTROL_ADDED))
            },
        ),
        label(move || removed_counts().map(|(_, removed)| format!("−{removed}")).unwrap_or_default())
            .style(move |s| {
                s.margin_left(4.0)
                    .color(config.get().color(LapceColor::SOURCE_CONTROL_REMOVED))
            }),
    ))
    .style(move |s| s.flex_shrink(0.0_f32).apply_if(counts().is_none(), |s| s.hide()))
}

/// Opening a file of the window's change (from the explorer, Quick Open): the change, base ↔ head side by side; a new
/// file with its lines marked as added. `false` for any other file, which opens as usual.
pub fn open_changed_file(window_tab: &WindowTabData, path: &Path) -> bool {
    let Some((change, file)) = window_tab.review.change.file_at(path) else {
        return false;
    };
    let line = change.files.get(&file).and_then(|file| file.first_line);
    code::compare(window_tab, Arc::clone(&change.review), file, line);
    true
}

/// The patches of a review's directory, parsed.
fn read_patches(review: &Review) -> Vec<PatchFile> {
    let Some(bundle) = review.bundle.as_ref() else {
        return Vec::new();
    };
    review_bundle::service::patches_of(Path::new(&bundle.dir))
        .into_iter()
        .flat_map(|patch| match std::fs::read_to_string(&patch) {
            Ok(text) => parse_patch(&text),
            Err(error) => {
                tracing::error!("could not read {}: {error}", patch.display());
                Vec::new()
            }
        })
        .collect()
}

/// The reviewer's **Changed files only** choices by review id, kept in the app's data folder.
fn choices_file() -> Option<PathBuf> {
    Directory::data_local_directory().map(|folder| folder.join("review-explorer.json"))
}

fn read_choices() -> HashMap<String, bool> {
    choices_file()
        .and_then(|file| std::fs::read_to_string(file).ok())
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

fn write_choices(choices: &HashMap<String, bool>) {
    let Some(file) = choices_file() else {
        return;
    };
    let written = serde_json::to_string(choices)
        .map_err(std::io::Error::other)
        .and_then(|text| std::fs::write(&file, text));
    if let Err(error) = written {
        tracing::error!("could not write {}: {error}", file.display());
    }
}
