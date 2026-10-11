//! NU — content that is not UTF-8.

/// **NU-03 — files git converts with `working-tree-encoding`.** `.gitattributes` says
/// [`ATTRS`]: `*.txt` files are UTF-16 LE with a byte order mark in the working tree,
/// UTF-8 in the index. `t.txt` is committed (`a`, `z`); the user's editor added a line
/// `ä`, and wrote a new file `n.txt` (`x`, `ö`) — both in UTF-16. The user discards the
/// added line of `t.txt` and stages only the line `ö` of the untracked `n.txt`.
///
/// Expected ([`expect`]): `t.txt` on disk is UTF-16 again, as committed; the index has
/// `n.txt` with only `ö`, in UTF-8 — what `git add` makes of it.
pub mod working_tree_encoding {
    use std::path::Path;

    use crate::expect::{Checks, Expect};
    use crate::ext::{Encoding, encode};
    use crate::fixture::{Fixture, put};

    pub const ATTRS: &str = "*.txt working-tree-encoding=UTF-16LE-BOM\n";
    /// The line of `t.txt` to discard, the line of `n.txt` to stage.
    pub const DISCARDED: &str = "ä";
    pub const STAGED: &str = "ö";

    /// `text` as the working tree holds it (UTF-16 LE with a byte order mark).
    pub fn utf16(text: &str) -> Vec<u8> {
        encode(text, Encoding::Utf16Le)
    }

    /// `.gitattributes` and `t.txt` committed (the checkout writes it as UTF-16); the
    /// edit and the new file written as UTF-16. Three git processes.
    pub fn fixture(dir: &Path) {
        let mut fx = Fixture::new();
        fx.commit("main", "init", &[put(".gitattributes", ATTRS), put("t.txt", "a\nz\n")]);
        fx.write_bytes("t.txt", &utf16("a\nä\nz\n"));
        fx.write_bytes("n.txt", &utf16("x\nö\n"));
        fx.build(dir);
    }

    /// After the discard and the stage. Two git processes.
    pub fn expect(dir: &Path) -> Result<(), String> {
        let e = Expect::new(dir);
        let mut c = Checks::new("NU-03 after discarding a line and staging another");
        let ix = e.blobs(&[":t.txt", ":n.txt"]);
        c.bytes("index t.txt (UTF-8)", ix[0].as_deref(), Some("a\nz\n".as_bytes()));
        c.bytes("index n.txt (UTF-8)", ix[1].as_deref(), Some("ö\n".as_bytes()));
        c.bytes("t.txt on disk (UTF-16)", e.file("t.txt").as_deref(), Some(&utf16("a\nz\n")));
        c.bytes("n.txt on disk (UTF-16)", e.file("n.txt").as_deref(), Some(&utf16("x\nö\n")));
        let mut st = e.status_lines();
        st.sort();
        c.eq("status", st, vec!["AM n.txt".to_string()]);
        c.done()
    }
}
