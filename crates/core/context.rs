use lru::LruCache;
use parking_lot::RwLock;
use std::io::Write;
use std::num::NonZero;
use std::sync::Arc;

use crate::cli::Commands;
use crate::document::SearchDocument;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OutputMode {
    Pretty,
    Json,
}

/// Cache entry for SearchDocument with validation metadata.
#[derive(Clone)]
struct SearchDocCacheEntry {
    search_doc: SearchDocument,
    mtime: u64,
    size: u64,
    content_hash: u64,
}

/// Thread-safe, LRU-cached SearchDocument cache for grep optimization.
#[derive(Clone)]
pub struct SearchDocCache {
    inner: Arc<RwLock<LruCache<String, SearchDocCacheEntry>>>,
}

impl SearchDocCache {
    pub fn new(capacity: usize) -> Self {
        // Ensure capacity is at least 1 since NonZero::new(0) returns None
        let non_zero_capacity = NonZero::new(capacity.max(1)).unwrap();
        Self {
            inner: Arc::new(RwLock::new(LruCache::new(non_zero_capacity))),
        }
    }

    /// Get a cached SearchDocument if still valid.
    pub fn get(
        &self,
        path: &std::path::Path,
        mtime: u64,
        size: u64,
        content_hash: u64,
    ) -> Option<SearchDocument> {
        let key = path.display().to_string();
        let mut cache = self.inner.write();
        if let Some(entry) = cache.get(&key) {
            if entry.mtime == mtime && entry.size == size && entry.content_hash == content_hash {
                return Some(entry.search_doc.clone());
            }
        }
        None
    }

    /// Insert a SearchDocument into the cache.
    pub fn put(
        &self,
        path: &std::path::Path,
        search_doc: SearchDocument,
        mtime: u64,
        size: u64,
        content_hash: u64,
    ) {
        let key = path.display().to_string();
        let mut cache = self.inner.write();
        cache.put(
            key,
            SearchDocCacheEntry {
                search_doc,
                mtime,
                size,
                content_hash,
            },
        );
    }
}

pub struct CommandContext<'a, W: Write, E: Write> {
    stdout: &'a mut W,
    stderr: &'a mut E,
    output_mode: OutputMode,
    pub search_doc_cache: SearchDocCache,
}

impl<'a, W: Write, E: Write> CommandContext<'a, W, E> {
    pub fn new(
        stdout: &'a mut W,
        stderr: &'a mut E,
        output_mode: OutputMode,
        search_doc_cache: SearchDocCache,
    ) -> Self {
        Self {
            stdout,
            stderr,
            output_mode,
            search_doc_cache,
        }
    }

    pub fn stdout(&mut self) -> &mut W {
        self.stdout
    }

    pub fn stderr(&mut self) -> &mut E {
        self.stderr
    }

    pub fn output_mode(&self) -> OutputMode {
        self.output_mode
    }
}

pub fn output_mode_for(command: &Commands) -> OutputMode {
    match command {
        Commands::Read(cmd) => flag_mode(cmd.json),
        Commands::Index(cmd) => flag_mode(cmd.json),
        Commands::Edit(cmd) => flag_mode(cmd.json),
        Commands::Verify(cmd) => flag_mode(cmd.json),
        Commands::Grep(cmd) => flag_mode(cmd.json),
        Commands::Annotate(cmd) => flag_mode(cmd.json),
        Commands::Insert(cmd) => flag_mode(cmd.json),
        Commands::Delete(cmd) => flag_mode(cmd.json),
        Commands::Patch(cmd) => flag_mode(cmd.json),
        Commands::Indent(cmd) => flag_mode(cmd.json),
        Commands::FindBlock(cmd) => flag_mode(cmd.json),
        Commands::Stats(cmd) => flag_mode(cmd.json),
        Commands::Doctor(cmd) => flag_mode(cmd.json),
        Commands::Workflows(cmd) => flag_mode(cmd.json),
        Commands::FromDiff(cmd) => flag_mode(cmd.json),
        Commands::MergePatches(cmd) => flag_mode(cmd.json),
        Commands::Watch(cmd) => flag_mode(cmd.json),
        Commands::WatchCapabilities(cmd) => flag_mode(cmd.json),
        Commands::Swap(_)
        | Commands::Move(_)
        | Commands::Explode(_)
        | Commands::Implode(_)
        | Commands::InstallMcp(_)
        | Commands::Mcp(_)
        | Commands::Daemon
        | Commands::Map(_)
        | Commands::Outline(_)
        | Commands::Symbol(_)
        | Commands::Callers(_)
        | Commands::Callees(_)
        | Commands::Deps(_) => OutputMode::Pretty,
    }
}

fn flag_mode(json: bool) -> OutputMode {
    if json {
        OutputMode::Json
    } else {
        OutputMode::Pretty
    }
}

#[cfg(test)]
mod tests {
    use super::{OutputMode, output_mode_for};
    use crate::cli::{
        Commands, DeleteCmd, DoctorCmd, EditCmd, ExplodeCmd, ImplodeCmd, IndentCmd, InsertCmd,
        ReadCmd, WatchCapabilitiesCmd, WatchCmd, WorkflowsCmd,
    };
    use std::path::PathBuf;

    #[test]
    fn uses_json_mode_when_command_requests_it() {
        let command = Commands::Read(ReadCmd {
            file: PathBuf::from("demo.txt"),
            anchor: Vec::new(),
            context: 5,
            json: true,
        });

        assert_eq!(output_mode_for(&command), OutputMode::Json);
    }

    #[test]
    fn uses_pretty_mode_when_json_flag_is_false() {
        let command = Commands::Edit(EditCmd {
            file: PathBuf::from("demo.txt"),
            anchor: "1:aa".into(),
            content: "new".into(),
            dry_run: false,
            receipt: false,
            audit_log: None,
            expect_mtime: None,
            expect_inode: None,
            json: false,
        });

        assert_eq!(output_mode_for(&command), OutputMode::Pretty);
    }

    #[test]
    fn defaults_to_pretty_for_commands_without_json_flag() {
        let command = Commands::Explode(ExplodeCmd {
            file: PathBuf::from("demo.txt"),
            out: PathBuf::from("out"),
            force: false,
        });

        assert_eq!(output_mode_for(&command), OutputMode::Pretty);
    }

    #[test]
    fn implode_defaults_to_pretty_mode() {
        let command = Commands::Implode(ImplodeCmd {
            dir: PathBuf::from("exploded"),
            out: PathBuf::from("demo.txt"),
            dry_run: true,
        });

        assert_eq!(output_mode_for(&command), OutputMode::Pretty);
    }

    #[test]
    fn supports_json_mode_for_watch() {
        let command = Commands::Watch(WatchCmd {
            file: PathBuf::from("demo.txt"),
            once: false,
            continuous: true,
            json: true,
        });

        assert_eq!(output_mode_for(&command), OutputMode::Json);
    }

    #[test]
    fn supports_json_mode_for_watch_capabilities() {
        let command = Commands::WatchCapabilities(WatchCapabilitiesCmd { json: true });

        assert_eq!(output_mode_for(&command), OutputMode::Json);
    }

    #[test]
    fn supports_json_mode_for_workflows() {
        let command = Commands::Workflows(WorkflowsCmd {
            root: Some(PathBuf::from(".")),
            json: true,
        });

        assert_eq!(output_mode_for(&command), OutputMode::Json);
    }

    #[test]
    fn supports_json_mode_for_insert() {
        let command = Commands::Insert(InsertCmd {
            file: PathBuf::from("demo.txt"),
            anchor: "1:aa".into(),
            content: "new".into(),
            before: false,
            dry_run: true,
            receipt: false,
            audit_log: None,
            expect_mtime: None,
            expect_inode: None,
            json: true,
        });

        assert_eq!(output_mode_for(&command), OutputMode::Json);
    }

    #[test]
    fn supports_json_mode_for_indent() {
        let command = Commands::Indent(IndentCmd {
            file: PathBuf::from("demo.txt"),
            range: "1:aa..2:bb".into(),
            amount: "+2".into(),
            dry_run: true,
            receipt: false,
            audit_log: None,
            expect_mtime: None,
            expect_inode: None,
            json: true,
        });

        assert_eq!(output_mode_for(&command), OutputMode::Json);
    }

    #[test]
    fn supports_json_mode_for_delete() {
        let command = Commands::Delete(DeleteCmd {
            file: PathBuf::from("demo.txt"),
            anchor: "1:aa".into(),
            dry_run: true,
            receipt: false,
            audit_log: None,
            expect_mtime: None,
            expect_inode: None,
            json: true,
        });

        assert_eq!(output_mode_for(&command), OutputMode::Json);
    }

    #[test]
    fn supports_json_mode_for_doctor() {
        let command = Commands::Doctor(DoctorCmd {
            file: PathBuf::from("demo.txt"),
            json: true,
        });

        assert_eq!(output_mode_for(&command), OutputMode::Json);
    }
}
