//! Reading a whole start.gg bracket and shaping it into a tree.
//!
//! `fetch` pulls every set over the unauthenticated website endpoint (no
//! token); `layout` turns that flat list into ordered winners/losers columns.
//! Neither touches start.gg's mutations — assigning, starting and reporting
//! stay on the operator's authenticated client in `station-core`.

pub mod fetch;
pub mod layout;
