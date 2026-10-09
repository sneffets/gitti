//! "Search in Changes" (experimental): find the commits whose *changed lines*
//! contain all the typed words — the commit list is filtered to them live, and the shown
//! commit's files / hunks to the matching ones.
//!
//! - [`query`]: words / `"phrases"`, smart case, strict mode (the word must touch the
//!   changed part of its line: the intra-line marks), refinement while typing on;
//! - [`score`]: ranking by proximity — all words in one line > close together in one file
//!   > in one file > spread over the commit — and the "why" shown per hit;
//! - [`scan`]: the live scan over the loaded history, newest first, in parallel chunks of
//!   `git diff-tree --stdin -p -U0 -G…` (git's pickaxe as the prefilter), cancellable,
//!   with progress and a cap;
//! - [`session`]: the state both frontends keep (hits sorted, progress line, "More…");
//! - [`view`]: the shown commit's hunks limited to the matches, the match highlights.
//!
//! No index yet; an SQLite FTS index is a possible later step. Every query scans; typing
//! on narrows to the previous hits.

pub mod query;
pub mod scan;
pub mod score;
pub mod session;
pub mod view;

pub use query::{ChangeQuery, LineMatch, Term};
pub use scan::{ChangeHit, ScanEvent, ScanJob, Snippet, Targets};
pub use score::{Proximity, Tier};
pub use session::{ChangeSearch, Order, Status};
pub use view::{HunkFilter, filter_rows};
