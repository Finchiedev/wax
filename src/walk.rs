use itertools::{EitherOrBoth, Itertools as _, Position};
use regex::Regex;
use std::borrow::Cow;
use std::fs::{FileType, Metadata};
use std::path::{Component, Path, PathBuf};
use walkdir::{self, DirEntry, WalkDir};

use crate::capture::MatchedText;
use crate::token::{self, Boundedness, Token};
use crate::{CandidatePath, Glob, GlobError, PositionExt as _};

/// Describes errors that occur when matching a [`Glob`] against a directory
/// tree.
///
/// [`Glob`]: crate::Glob
pub use walkdir::Error as WalkError;

/// Traverses a directory tree via a `Walk` instance.
///
/// This macro emits an interruptable loop that executes a block of code
/// whenever a `WalkEntry` or error is encountered while traversing a directory
/// tree. The block may return from its function or otherwise interrupt and
/// subsequently resume the loop.
///
/// There are two expansions for this macro that correspond to the type
/// parameter of `Walk`: one for walking without negations and one for walking
/// with negations. Negations are considered separately to avoid branching where
/// it is not necessary. Moreover, terminal negations must arrest descent into
/// directories to avoid what could be substantial and unnecessary work.
///
/// Note that if the block attempts to emit a `WalkEntry` across a function
/// boundary, then the entry contents must be copied via `into_owned`.
macro_rules! walk {
    ((), $state:expr => |$entry:ident| $f:block) => {
        use itertools::EitherOrBoth::{Both, Left, Right};
        use itertools::Position::{First, Last, Middle, Only};

        // `while-let` avoids a mutable borrow of `walk`, which would prevent a
        // subsequent call to `skip_current_dir` within the loop body.
        #[allow(clippy::while_let_on_iterator)]
        #[allow(unreachable_code)]
        'walk: while let Some(entry) = $state.walk.next() {
            let entry = match entry {
                Ok(entry) => entry,
                Err(error) => {
                    let $entry = Err(error.into());
                    $f
                    continue; // May be unreachable.
                }
            };
            let path = entry
                .path()
                .strip_prefix(&$state.prefix)
                .expect("path is not in tree");
            for candidate in candidates(&entry, path, $state.components.iter()) {
                match candidate.as_tuple() {
                    (First(_) | Middle(_), Both(component, pattern)) => {
                        if !pattern.is_match(component.as_ref()) {
                            // Do not descend into directories that do not match
                            // the corresponding component pattern.
                            if entry.file_type().is_dir() {
                                $state.walk.skip_current_dir();
                            }
                            continue 'walk;
                        }
                    }
                    (Last(_) | Only(_), Both(component, pattern)) => {
                        if pattern.is_match(component.as_ref()) {
                            let path = CandidatePath::from(path);
                            if let Some(matched) =
                                $state.pattern.captures(path.as_ref()).map(MatchedText::from)
                            {
                                let $entry = Ok(WalkEntry {
                                    entry: Cow::Borrowed(&entry),
                                    matched,
                                });
                                $f
                            }
                        }
                        else {
                            // Do not descend into directories that do not match
                            // the corresponding component pattern.
                            if entry.file_type().is_dir() {
                                $state.walk.skip_current_dir();
                            }
                        }
                        continue 'walk;
                    }
                    (_, Left(_component)) => {
                        let path = CandidatePath::from(path);
                        if let Some(matched) =
                            $state.pattern.captures(path.as_ref()).map(MatchedText::from)
                        {
                            let $entry = Ok(WalkEntry {
                                entry: Cow::Borrowed(&entry),
                                matched,
                            });
                            $f
                        }
                        continue 'walk;
                    }
                    (_, Right(_pattern)) => {
                        continue 'walk;
                    }
                }
            }
            // If the loop is not entered, check for a match. This may indicate
            // that the `Glob` is empty and a single invariant path may be
            // matched.
            let path = CandidatePath::from(path);
            if let Some(matched) = $state.pattern.captures(path.as_ref()).map(MatchedText::from) {
                let $entry = Ok(WalkEntry {
                    entry: Cow::Borrowed(&entry),
                    matched,
                });
                $f
            }
        }
    };
    (Negation, $state:expr => |$entry:ident| $f:block) => {
        use itertools::EitherOrBoth::{Both, Left, Right};
        use itertools::Position::{First, Last, Middle, Only};

        // `while-let` avoids a mutable borrow of `walk`, which would prevent a
        // subsequent call to `skip_current_dir` within the loop body.
        #[allow(clippy::while_let_on_iterator)]
        #[allow(unreachable_code)]
        'walk: while let Some(entry) = $state.walk.next() {
            let entry = match entry {
                Ok(entry) => entry,
                Err(error) => {
                    let $entry = Err(error.into());
                    $f
                    continue; // May be unreachable.
                }
            };
            let path = entry
                .path()
                .strip_prefix(&$state.prefix)
                .expect("path is not in tree");
            let candidates = candidates(&entry, path, $state.components.iter());
            let path = CandidatePath::from(path);
            if $state.negation.terminal.is_match(path.as_ref()) {
                // Do not descend into directories that match the terminal
                // negation.
                if entry.file_type().is_dir() {
                    $state.walk.skip_current_dir();
                }
                continue 'walk;
            }
            if $state.negation.nonterminal.is_match(path.as_ref()) {
                continue 'walk;
            }
            for candidate in candidates {
                match candidate.as_tuple() {
                    (First(_) | Middle(_), Both(component, pattern)) => {
                        if !pattern.is_match(component.as_ref()) {
                            // Do not descend into directories that do not match
                            // the corresponding component pattern.
                            if entry.file_type().is_dir() {
                                $state.walk.skip_current_dir();
                            }
                            continue 'walk;
                        }
                    }
                    (Last(_) | Only(_), Both(component, pattern)) => {
                        if pattern.is_match(component.as_ref()) {
                            if let Some(matched) =
                                $state.pattern.captures(path.as_ref()).map(MatchedText::from)
                            {
                                let $entry = Ok(WalkEntry {
                                    entry: Cow::Borrowed(&entry),
                                    matched,
                                });
                                $f
                            }
                        }
                        else {
                            // Do not descend into directories that do not match
                            // the corresponding component pattern.
                            if entry.file_type().is_dir() {
                                $state.walk.skip_current_dir();
                            }
                        }
                        continue 'walk;
                    }
                    (_, Left(_component)) => {
                        if let Some(matched) =
                            $state.pattern.captures(path.as_ref()).map(MatchedText::from)
                        {
                            let $entry = Ok(WalkEntry {
                                entry: Cow::Borrowed(&entry),
                                matched,
                            });
                            $f
                        }
                        continue 'walk;
                    }
                    (_, Right(_pattern)) => {
                        continue 'walk;
                    }
                }
            }
            // If the loop is not entered, check for a match. This may indicate
            // that the `Glob` is empty and a single invariant path may be
            // matched.
            if let Some(matched) = $state.pattern.captures(path.as_ref()).map(MatchedText::from) {
                let $entry = Ok(WalkEntry {
                    entry: Cow::Borrowed(&entry),
                    matched,
                });
                $f
            }
        }
    };
}

// This trait is used to provide a uniform API for `Walk::for_each`. Rather than
// implementing `for_each` for `Walk<'_, ()>` and `Walk<'_, Negation>`, a
// general implementation is used with a bound on this trait. This trait will
// always be implemented for the exposed `Walk` types, so client code can
// effectively ignore this bound.
pub trait ForEach {
    fn for_each(self, f: impl FnMut(Result<WalkEntry, WalkError>));
}

#[derive(Clone, Debug)]
pub struct Negation {
    terminal: Regex,
    nonterminal: Regex,
}

impl Negation {
    pub fn try_from_patterns<'n, P>(
        patterns: impl IntoIterator<Item = P>,
    ) -> Result<Self, GlobError<'n>>
    where
        P: TryInto<Glob<'n>, Error = GlobError<'n>>,
    {
        let globs: Vec<_> = patterns
            .into_iter()
            .map(TryInto::try_into)
            .collect::<Result<_, _>>()?;
        // Partition the negation globs into terminals and nonterminals. A
        // terminal glob matches all sub-paths once it has matched and so
        // arrests the traversal into sub-directories. This is determined by
        // whether or not a glob is terminated with a tree wildcard.
        let (terminals, nonterminals) = globs.into_iter().partition::<Vec<_>, _>(is_terminal);
        Ok(Negation {
            terminal: crate::any::<Glob, _>(terminals).unwrap().regex,
            nonterminal: crate::any::<Glob, _>(nonterminals).unwrap().regex,
        })
    }
}

/// Configuration for interpreting symbolic links.
///
/// Determines how symbolic links are interpreted when traversing directory
/// trees using functions like [`Glob::walk`]. By default, symbolic links are
/// read as regular files and their targets are ignored.
///
/// [`Glob::walk`]: crate::Glob::walk
#[derive(Clone, Copy, Debug)]
pub enum LinkBehavior {
    /// Read the symbolic link file itself.
    ///
    /// This behavior reads the symbolic link as a regular file. The
    /// corresponding [`WalkEntry`] uses the path of the link file and its
    /// metadata describes the link file itself. The target is effectively
    /// ignored and traversal will **not** follow the link.
    ///
    /// [`WalkEntry`]: crate::WalkEntry
    ReadFile,
    /// Read the target of the symbolic link.
    ///
    /// This behavior reads the target of the symbolic link. The corresponding
    /// [`WalkEntry`] uses the path of the link file and its metadata describes
    /// the target. If the target is a directory, then traversal will follow the
    /// link and descend into the target.
    ///
    /// If a link is reentrant and forms a cycle, then an error will be emitted
    /// instead of a [`WalkEntry`] and traversal will not follow the link.
    ///
    /// [`WalkEntry`]: crate::WalkEntry
    ReadTarget,
}

impl Default for LinkBehavior {
    fn default() -> Self {
        LinkBehavior::ReadFile
    }
}

/// Configuration for matching [`Glob`]s against directory trees.
///
/// Determines the behavior of the traversal within a directory tree when using
/// functions like [`Glob::walk`]. `WalkBehavior` can be constructed via
/// conversions from types representing its fields. APIs generally accept `impl
/// Into<WalkBehavior>`, so these conversion can be used implicitly. When
/// constructed using such a conversion, `WalkBehavior` will use defaults for
/// any remaining fields.
///
/// # Examples
///
/// By default, symbolic links are interpreted as regular files and targets are
/// ignored. To read linked targets, use [`LinkBehavior::ReadTarget`].
///
/// ```rust
/// use wax::LinkBehavior;
///
/// for entry in wax::walk("**", ".", LinkBehavior::ReadTarget).unwrap() {
///     let entry = entry.unwrap();
///     // ...
/// }
/// ```
///
/// [`Glob`]: crate::Glob
/// [`Glob::walk`]: crate::Glob::walk
#[derive(Clone, Copy, Debug)]
pub struct WalkBehavior {
    // TODO: Consider using a dedicated type for this field. Using primitive
    //       types does not interact well with conversions used in `walk` APIs.
    //       For example, if another `usize` field is introduced, then the
    //       conversions become ambiguous and confusing.
    /// Maximum depth.
    ///
    /// Determines the maximum depth to which a directory tree will be traversed
    /// relative to the root. A depth of zero corresponds to the root and so
    /// using such a depth will yield at most one entry for the root.
    ///
    /// The default value is [`usize::MAX`].
    ///
    /// [`usize::MAX`]: usize::MAX
    pub depth: usize,
    /// Interpretation of symbolic links.
    ///
    /// Determines how symbolic links are interpreted when traversing a
    /// directory tree. See [`LinkBehavior`].
    ///
    /// The default value is [`LinkBehavior::ReadFile`].
    ///
    /// [`LinkBehavior`]: crate::LinkBehavior
    /// [`LinkBehavior::ReadFile`]: crate::LinkBehavior::ReadFile
    pub link: LinkBehavior,
}

/// Constructs a `WalkBehavior` using the following defaults:
///
/// | Field     | Description                       | Value                      |
/// |-----------|-----------------------------------|----------------------------|
/// | [`depth`] | Maximum depth.                    | [`usize::MAX`]             |
/// | [`link`]  | Interpretation of symbolic links. | [`LinkBehavior::ReadFile`] |
///
/// [`depth`]: crate::WalkBehavior::depth
/// [`link`]: crate::WalkBehavior::link
/// [`LinkBehavior::ReadFile`]: crate::LinkBehavior::ReadFile
/// [`usize::MAX`]: usize::MAX
impl Default for WalkBehavior {
    fn default() -> Self {
        WalkBehavior {
            depth: usize::MAX,
            link: Default::default(),
        }
    }
}

impl From<()> for WalkBehavior {
    fn from(_: ()) -> Self {
        Default::default()
    }
}

impl From<LinkBehavior> for WalkBehavior {
    fn from(link: LinkBehavior) -> Self {
        WalkBehavior {
            link,
            ..Default::default()
        }
    }
}

impl From<usize> for WalkBehavior {
    fn from(depth: usize) -> Self {
        WalkBehavior {
            depth,
            ..Default::default()
        }
    }
}

/// Iterator over files matching a [`Glob`] in a directory tree.
///
/// [`Glob`]: crate::Glob
#[derive(Debug)]
// This type is principally an iterator and is therefore lazy.
#[must_use]
pub struct Walk<'g, N = ()> {
    pattern: Cow<'g, Regex>,
    components: Vec<Regex>,
    negation: N,
    prefix: PathBuf,
    walk: walkdir::IntoIter,
}

impl<'g, N> Walk<'g, N> {
    fn compile<'t, I>(tokens: I) -> Vec<Regex>
    where
        I: IntoIterator<Item = &'t Token<'t>>,
        I::IntoIter: Clone,
    {
        let mut regexes = Vec::new();
        for component in token::components(tokens) {
            if component
                .tokens()
                .iter()
                .any(|token| token.has_component_boundary())
            {
                // Stop at component boundaries, such as tree wildcards or any
                // boundary within an alternative token.
                break;
            }
            else {
                regexes.push(Glob::compile(component.tokens().iter().cloned()));
            }
        }
        regexes
    }

    /// Clones any borrowed data into an owning instance.
    pub fn into_owned(self) -> Walk<'static, N> {
        let Walk {
            pattern,
            components,
            negation,
            prefix,
            walk,
        } = self;
        Walk {
            pattern: Cow::Owned(pattern.into_owned()),
            components,
            negation,
            prefix,
            walk,
        }
    }

    /// Calls a closure on each matched file or error.
    ///
    /// This function does not clone the contents of paths and captures when
    /// emitting entries and so may be more efficient than external iteration
    /// via [`Iterator`] (and [`Iterator::for_each`]), which must clone text.
    ///
    /// [`Iterator`]: std::iter::Iterator
    /// [`Iterator::for_each`]: std::iter::Iterator::for_each
    pub fn for_each(self, f: impl FnMut(Result<WalkEntry, WalkError>))
    where
        Self: ForEach,
    {
        ForEach::for_each(self, f)
    }
}

impl<'g> Walk<'g, ()> {
    /// Filters [`WalkEntry`]s against negated [`Glob`]s.
    ///
    /// This function creates an adaptor that discards [`WalkEntry`]s that match
    /// any of the given [`Glob`]s. This allows for broad negations while
    /// matching a [`Glob`] against a directory tree that cannot be achieved
    /// using a single glob expression.
    ///
    /// **This adaptor should be preferred over external iterator filtering
    /// (e.g., via [`Iterator::filter`]), because it does not walk directory
    /// trees if they match terminal negations.** For example, if the glob
    /// expression `**/private/**` is used as a negation, then this adaptor will
    /// not walk any directory trees rooted by a `private` directory. External
    /// filtering cannot interact with the traversal, and so may needlessly read
    /// sub-trees.
    ///
    /// Errors are not filtered, so if an error occurs reading a file at a path
    /// that would have been discarded, that error is still yielded by the
    /// iterator.
    ///
    /// # Errors
    ///
    /// Returns an error if any of the given patterns could not be converted
    /// into a [`Glob`]. If the given patterns are [`Glob`]s, then this function
    /// is infallible.
    ///
    /// # Examples
    ///
    /// Because glob expressions do not support general negations, it is
    /// sometimes impossible to express patterns that deny particular text. In
    /// such cases, `not` can be used to apply additional patterns as a filter.
    ///
    /// ```rust,no_run
    /// use wax::Glob;
    ///
    /// // Find image files, but not if they are beneath a directory with a name that
    /// // suggests that they are private.
    /// let glob = Glob::new("**/*.(?i){jpg,jpeg,png}").unwrap();
    /// for entry in glob
    ///     .walk(".", usize::MAX)
    ///     .not(["**/(?i)<.:0,1>private/**"])
    ///     .unwrap()
    /// {
    ///     let entry = entry.unwrap();
    ///     // ...
    /// }
    /// ```
    ///
    /// [`Glob`]: crate::Glob
    /// [`Iterator::filter`]: std::iter::Iterator::filter
    /// [`WalkEntry`]: crate::WalkEntry
    pub fn not<'n, P>(
        self,
        patterns: impl IntoIterator<Item = P>,
    ) -> Result<Walk<'g, Negation>, GlobError<'n>>
    where
        P: TryInto<Glob<'n>, Error = GlobError<'n>>,
    {
        let negation = Negation::try_from_patterns(patterns)?;
        let Walk {
            pattern,
            components,
            prefix,
            walk,
            ..
        } = self;
        Ok(Walk {
            pattern,
            components,
            negation,
            prefix,
            walk,
        })
    }
}

impl<'g> ForEach for Walk<'g, ()> {
    fn for_each(mut self, mut f: impl FnMut(Result<WalkEntry, WalkError>)) {
        walk!((), self => |entry| {
            f(entry);
        });
    }
}

impl<'g> ForEach for Walk<'g, Negation> {
    fn for_each(mut self, mut f: impl FnMut(Result<WalkEntry, WalkError>)) {
        walk!(Negation, self => |entry| {
            f(entry);
        });
    }
}

impl Iterator for Walk<'_, ()> {
    type Item = Result<WalkEntry<'static>, WalkError>;

    fn next(&mut self) -> Option<Self::Item> {
        walk!((), self => |entry| {
            return Some(entry.map(|entry: WalkEntry| entry.into_owned()));
        });
        None
    }
}

impl Iterator for Walk<'_, Negation> {
    type Item = Result<WalkEntry<'static>, WalkError>;

    fn next(&mut self) -> Option<Self::Item> {
        walk!(Negation, self => |entry| {
            return Some(entry.map(|entry: WalkEntry| entry.into_owned()));
        });
        None
    }
}

/// Describes a file matching a [`Glob`] in a directory tree.
///
/// [`Glob`]: crate::Glob
#[derive(Debug)]
pub struct WalkEntry<'e> {
    entry: Cow<'e, DirEntry>,
    matched: MatchedText<'e>,
}

impl<'e> WalkEntry<'e> {
    /// Clones any borrowed data into an owning instance.
    pub fn into_owned(self) -> WalkEntry<'static> {
        let WalkEntry { entry, matched } = self;
        WalkEntry {
            entry: Cow::Owned(entry.into_owned()),
            matched: matched.into_owned(),
        }
    }

    pub fn into_path(self) -> PathBuf {
        match self.entry {
            Cow::Borrowed(entry) => entry.path().to_path_buf(),
            Cow::Owned(entry) => entry.into_path(),
        }
    }

    /// Gets the path of the matched file.
    pub fn path(&self) -> &Path {
        self.entry.path()
    }

    /// Converts the entry to the matched [`CandidatePath`].
    ///
    /// This differs from `path` and `into_path`, and uses the same encoding and
    /// representation exposed by the matched text in `matched`.
    ///
    /// [`CandidatePath`]: crate::CandidatePath
    /// [`into_path`]: crate::WalkEntry::into_path
    /// [`matched`]: crate::WalkEntry::matched
    /// [`path`]: crate::WalkEntry::path
    pub fn to_candidate_path(&self) -> CandidatePath<'_> {
        self.path().into()
    }

    pub fn file_type(&self) -> FileType {
        self.entry.file_type()
    }

    pub fn metadata(&self) -> Result<Metadata, GlobError<'static>> {
        self.entry.metadata().map_err(From::from)
    }

    /// Gets the depth of the file from the root of the directory tree.
    pub fn depth(&self) -> usize {
        self.entry.depth()
    }

    /// Gets the matched text in the path of the file.
    pub fn matched(&self) -> &MatchedText<'e> {
        &self.matched
    }
}

pub fn walk<'g>(
    glob: &'g Glob<'_>,
    directory: impl AsRef<Path>,
    behavior: impl Into<WalkBehavior>,
) -> Walk<'g, ()> {
    let directory = directory.as_ref();
    let WalkBehavior { depth, link } = behavior.into();
    // The directory tree is traversed from `root`, which may include an
    // invariant prefix from the glob pattern. `Walk` patterns are only
    // applied to path components following the `prefix` (distinct from the
    // glob pattern prefix) in `root`.
    let (root, prefix, depth) = token::invariant_prefix_path(glob.tokenized.tokens())
        .map(|prefix| {
            let root = directory.join(&prefix).into();
            if prefix.is_absolute() {
                // Absolute paths replace paths with which they are joined,
                // in which case there is no prefix.
                (root, PathBuf::new().into(), depth)
            }
            else {
                // TODO: If the depth is exhausted by an invariant prefix
                //       path, then `Walk` should yield no entries. This
                //       computes a depth of zero when this occurs, so
                //       entries may still be yielded.
                // `depth` is relative to the input `directory`, so count
                // any components added by an invariant prefix path from the
                // glob.
                let depth = depth.saturating_sub(prefix.components().count());
                (root, directory.into(), depth)
            }
        })
        .unwrap_or_else(|| {
            let root = Cow::from(directory);
            (root.clone(), root, depth)
        });
    let components = Walk::<()>::compile(glob.tokenized.tokens());
    Walk {
        pattern: Cow::Borrowed(&glob.regex),
        components,
        negation: (),
        prefix: prefix.into_owned(),
        walk: WalkDir::new(root)
            .follow_links(match link {
                LinkBehavior::ReadFile => false,
                LinkBehavior::ReadTarget => true,
            })
            .max_depth(depth)
            .into_iter(),
    }
}

fn candidates<'e>(
    entry: &'e DirEntry,
    path: &'e Path,
    patterns: impl IntoIterator<Item = &'e Regex>,
) -> impl Iterator<Item = Position<EitherOrBoth<CandidatePath<'e>, &'e Regex>>> {
    let depth = entry.depth().saturating_sub(1);
    path.components()
        .skip(depth)
        .filter_map(|component| match component {
            Component::Normal(component) => Some(CandidatePath::from(component)),
            _ => None,
        })
        .zip_longest(patterns.into_iter().skip(depth))
        .with_position()
}

/// Returns `true` if the [`Glob`] is terminal.
///
/// A [`Glob`] is terminal if its final component has unbounded depth and
/// unbounded variance. When walking a directory tree, such an expression allows
/// a matching directory to be ignored when used as a negation, because the
/// negating expression matches any and all sub-paths.
///
/// See [`Negation`].
///
/// [`Glob`]: crate::Glob
/// [`Negation`]: crate::walk::Negation
fn is_terminal(glob: &Glob<'_>) -> bool {
    let component = token::components(glob.tokenized.tokens()).last();
    matches!(
        component.map(|component| { (component.depth(), component.variance().boundedness(),) }),
        Some((Boundedness::Open, Boundedness::Open)),
    )
}

#[cfg(test)]
mod tests {
    use crate::walk;
    use crate::Glob;

    #[test]
    fn query_terminal_glob() {
        assert!(walk::is_terminal(&Glob::new("**").unwrap()));
        assert!(walk::is_terminal(&Glob::new("a/**").unwrap()));
        assert!(walk::is_terminal(&Glob::new("a/<*/>*").unwrap()));
        assert!(walk::is_terminal(&Glob::new("a/<<?>/>*").unwrap()));

        assert!(!walk::is_terminal(&Glob::new("a/**/b").unwrap()));
        assert!(!walk::is_terminal(&Glob::new("a/*").unwrap()));
        assert!(!walk::is_terminal(&Glob::new("a/<?>").unwrap()));
        assert!(!walk::is_terminal(&Glob::new("a</**/b>").unwrap()));
        assert!(!walk::is_terminal(&Glob::new("**/a").unwrap()));
        assert!(!walk::is_terminal(&Glob::new("").unwrap()));
    }
}
