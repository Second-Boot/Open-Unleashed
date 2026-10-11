# The project's user-facing name, sourced by the packaging scripts (the game's window title is
# crates/porsche/src/brand.rs). A working name: change both together. It names no trademark of the
# original game's owners; the READMEs say which game it runs.
APP_ID=OpenUnleashed
APP_TITLE=OpenUnleashed
BUNDLE_ID=dev.openunleashed.OpenUnleashed
# @TITLE@ in a README template -> the title
brand_readme() { sed "s/@TITLE@/$APP_TITLE/g; s/@APP@/$APP_ID/g" "$1"; }
