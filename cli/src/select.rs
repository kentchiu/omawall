use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::error::Result;
use crate::state::{Disliked, Favorite, Favorites, QueryCursor, Seen, push_unique};
use crate::wallhaven::{self, Item, Query, Transport};

const MAX_PAGE_FETCHES: usize = 3;

/// xorshift64 seeded from time and pid; good enough for shuffling sources.
pub struct Rng(u64);

impl Rng {
    pub fn from_time() -> Self {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0);
        Self::seeded(nanos ^ (u64::from(std::process::id()) << 32))
    }

    pub fn seeded(seed: u64) -> Self {
        Self(if seed == 0 {
            0x9E37_79B9_7F4A_7C15
        } else {
            seed
        })
    }

    fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }

    /// Fisher–Yates shuffle.
    pub fn shuffle<T>(&mut self, items: &mut [T]) {
        for i in (1..items.len()).rev() {
            let j = (self.next_u64() % (i as u64 + 1)) as usize;
            items.swap(i, j);
        }
    }
}

/// What the selection may not pick.
pub struct Exclusions<'a> {
    pub disliked: &'a Disliked,
    pub current: Option<&'a str>,
}

/// Outcome of scanning one tag query.
pub struct TagPick {
    pub item: Option<Item>,
    /// `meta.total` of the last fetched page, if any.
    pub total: Option<u64>,
}

/// Walks the query cursor (at most 3 fetches, one reset past `last_page`) and
/// records the picked id in the cursor and in `seen.all`.
pub fn pick_tag(
    transport: &dyn Transport,
    query: &Query,
    seen: &mut Seen,
    ex: &Exclusions,
) -> Result<TagPick> {
    let cursor = seen
        .queries
        .entry(query.key())
        .or_insert_with(|| QueryCursor {
            page: 1,
            ids: Vec::new(),
        });
    let mut page = cursor.page.max(1);
    let mut last_page: Option<u64> = None;
    let mut reset_done = false;
    let mut total = None;
    let mut fetches = 0;
    loop {
        if last_page.is_some_and(|last| page > last) {
            if reset_done {
                break;
            }
            reset_done = true;
            page = 1;
            cursor.ids.clear();
        }
        if fetches == MAX_PAGE_FETCHES {
            break;
        }
        fetches += 1;
        let result = wallhaven::search(transport, query, page)?;
        total = Some(result.total);
        if result.total == 0 {
            break;
        }
        last_page = Some(result.last_page);
        let found = result.items.into_iter().find(|item| {
            !cursor.ids.contains(&item.id)
                && !ex.disliked.contains(&item.id)
                && ex.current != Some(item.id.as_str())
                && item.file_size <= wallhaven::FULL_LIMIT
        });
        if let Some(item) = found {
            cursor.page = page;
            cursor.ids.push(item.id.clone());
            push_unique(&mut seen.all, &item.id);
            return Ok(TagPick {
                item: Some(item),
                total,
            });
        }
        page += 1;
    }
    cursor.page = page;
    Ok(TagPick { item: None, total })
}

fn fav_usable(fav: &Favorite, current: Option<&str>) -> bool {
    current != Some(fav.id.as_str()) && Path::new(&fav.path).is_file()
}

/// `favs` is a candidate only when a favorite other than the current exists.
pub fn has_other_favorite(favorites: &Favorites, current: Option<&str>) -> bool {
    favorites.items.iter().any(|f| fav_usable(f, current))
}

/// First favorite (newest first) not yet shown; clears `seen.favs` once when exhausted.
pub fn pick_fav(favorites: &Favorites, seen: &mut Seen, current: Option<&str>) -> Option<Favorite> {
    for attempt in 0..2 {
        let found = favorites
            .items
            .iter()
            .find(|f| !seen.favs.contains(&f.id) && fav_usable(f, current));
        if let Some(fav) = found {
            push_unique(&mut seen.favs, &fav.id);
            push_unique(&mut seen.all, &fav.id);
            return Some(fav.clone());
        }
        if attempt == 0 {
            seen.favs.clear();
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::FakeTransport;
    use crate::wallhaven::tests::search_json;

    fn query() -> Query {
        Query {
            tag: 711,
            categories: "100",
            color: None,
        }
    }

    fn page_url(page: u64) -> String {
        query().url(page)
    }

    fn no_ex(disliked: &Disliked) -> Exclusions<'_> {
        Exclusions {
            disliked,
            current: None,
        }
    }

    #[test]
    fn shuffle_is_permutation() {
        let mut rng = Rng::seeded(42);
        let mut v: Vec<u32> = (0..10).collect();
        rng.shuffle(&mut v);
        let mut sorted = v.clone();
        sorted.sort();
        assert_eq!(sorted, (0..10).collect::<Vec<_>>());
    }

    #[test]
    fn cursor_advances_within_and_across_pages() {
        let t = FakeTransport::default();
        t.add(&page_url(1), 200, search_json(&["aa1", "aa2"], 2, 4));
        t.add(&page_url(2), 200, search_json(&["bb1", "bb2"], 2, 4));
        let mut seen = Seen::default();
        let disliked = Disliked::default();
        let mut picks = Vec::new();
        for _ in 0..4 {
            let pick = pick_tag(&t, &query(), &mut seen, &no_ex(&disliked)).unwrap();
            assert_eq!(pick.total, Some(4));
            picks.push(pick.item.unwrap().id);
        }
        assert_eq!(picks, ["aa1", "aa2", "bb1", "bb2"]);
        let cursor = &seen.queries["tag:711|cat:100|color:none"];
        assert_eq!(cursor.page, 2);
        assert_eq!(seen.all, ["aa1", "aa2", "bb1", "bb2"]);
    }

    #[test]
    fn page_exhaustion_resets_once() {
        let t = FakeTransport::default();
        t.add(&page_url(1), 200, search_json(&["aa1", "aa2"], 2, 4));
        t.add(&page_url(2), 200, search_json(&["bb1", "bb2"], 2, 4));
        let mut seen = Seen::default();
        seen.queries.insert(
            query().key(),
            QueryCursor {
                page: 2,
                ids: vec!["aa1".into(), "aa2".into(), "bb1".into(), "bb2".into()],
            },
        );
        let disliked = Disliked::default();
        let pick = pick_tag(&t, &query(), &mut seen, &no_ex(&disliked)).unwrap();
        assert_eq!(pick.item.unwrap().id, "aa1");
        let cursor = &seen.queries[&query().key()];
        assert_eq!(
            (cursor.page, cursor.ids.clone()),
            (1, vec!["aa1".to_string()])
        );
        // page 2 (nothing new) → past last_page → reset → page 1.
        assert_eq!(t.calls(), [page_url(2), page_url(1)]);
    }

    #[test]
    fn skips_disliked_and_current_and_gives_up() {
        let t = FakeTransport::default();
        t.add(&page_url(1), 200, search_json(&["aa1", "aa2"], 1, 2));
        t.add(&page_url(2), 200, search_json(&[], 1, 2));
        let mut seen = Seen::default();
        let disliked = Disliked {
            version: 1,
            ids: vec!["aa1".into()],
        };
        let ex = Exclusions {
            disliked: &disliked,
            current: Some("aa2"),
        };
        let pick = pick_tag(&t, &query(), &mut seen, &ex).unwrap();
        assert!(pick.item.is_none());
        assert_eq!(t.calls().len(), 2, "one reset retry, then stop");

        let ex = Exclusions {
            disliked: &disliked,
            current: None,
        };
        assert_eq!(
            pick_tag(&t, &query(), &mut seen, &ex)
                .unwrap()
                .item
                .unwrap()
                .id,
            "aa2"
        );
    }

    #[test]
    fn at_most_three_fetches() {
        let t = FakeTransport::default();
        for p in 1..=5 {
            t.add(&page_url(p), 200, search_json(&["dd1"], 5, 5));
        }
        let mut seen = Seen::default();
        let disliked = Disliked {
            version: 1,
            ids: vec!["dd1".into()],
        };
        let pick = pick_tag(&t, &query(), &mut seen, &no_ex(&disliked)).unwrap();
        assert!(pick.item.is_none());
        assert_eq!(t.calls().len(), 3);
        assert_eq!(seen.queries[&query().key()].page, 4);
    }

    #[test]
    fn empty_query_stops_after_one_fetch() {
        let t = FakeTransport::default();
        t.add(&page_url(1), 200, search_json(&[], 0, 0));
        let mut seen = Seen::default();
        let disliked = Disliked::default();
        let pick = pick_tag(&t, &query(), &mut seen, &no_ex(&disliked)).unwrap();
        assert!(pick.item.is_none());
        assert_eq!(pick.total, Some(0));
        assert_eq!(t.calls().len(), 1);
    }

    #[test]
    fn favs_source_order_and_reset() {
        let dir = crate::testutil::temp_dir("select-favs");
        let fav = |id: &str| {
            let path = dir.join(format!("wallhaven-{id}.jpg"));
            std::fs::write(&path, crate::testutil::JPEG).unwrap();
            Favorite {
                id: id.into(),
                path: path.display().to_string(),
                thumb: None,
                resolution: "3840x2160".into(),
                favorites: 1,
                file_size: 4,
                added_at: 0,
            }
        };
        let mut missing = fav("gone");
        missing.path = dir.join("nope.jpg").display().to_string();
        let favorites = Favorites {
            version: 1,
            items: vec![fav("new"), missing, fav("old")],
        };
        let mut seen = Seen::default();

        assert!(has_other_favorite(&favorites, Some("new")));
        assert!(!has_other_favorite(
            &Favorites {
                version: 1,
                items: vec![fav("new")]
            },
            Some("new")
        ));

        let ids: Vec<_> = (0..3)
            .map(|_| pick_fav(&favorites, &mut seen, Some("old")).unwrap().id)
            .collect();
        // "old" is current and "gone" has no file: only "new" is usable, re-shown after reset.
        assert_eq!(ids, ["new", "new", "new"]);
        let a = pick_fav(&favorites, &mut seen, None).unwrap().id;
        let b = pick_fav(&favorites, &mut seen, None).unwrap().id;
        assert_eq!((a.as_str(), b.as_str()), ("old", "new"));
        assert!(pick_fav(&Favorites::default(), &mut seen, None).is_none());
    }
}
