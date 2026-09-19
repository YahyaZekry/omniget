//! Tile coordinates and the tile palette shared with the art atlas.

use serde::{Deserialize, Serialize};

/// Whole-tile coordinate. The map is unbounded in principle; a `MapDef` fixes
/// the rectangle that actually has chunks.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default, Hash, Debug)]
pub struct Tile {
    pub x: i32,
    pub y: i32,
}

impl Tile {
    pub const fn new(x: i32, y: i32) -> Tile {
        Tile { x, y }
    }

    /// Chebyshev distance: the grid allows diagonals, so this is the number of
    /// steps, and it is what [`crate::Interest`] filters on.
    pub const fn chebyshev(self, other: Tile) -> u32 {
        let dx = (self.x - other.x).unsigned_abs();
        let dy = (self.y - other.y).unsigned_abs();
        if dx > dy {
            dx
        } else {
            dy
        }
    }

    /// Squared euclidean distance in tiles, for radius comparisons without a
    /// square root.
    pub const fn dist2(self, other: Tile) -> u64 {
        let dx = (self.x - other.x) as i64;
        let dy = (self.y - other.y) as i64;
        (dx * dx + dy * dy) as u64
    }
}

impl Serialize for Tile {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        // JSON maps write tiles as [x, y]; two numbers read better than an
        // object in a file with thousands of them.
        [self.x, self.y].serialize(s)
    }
}

impl<'de> Deserialize<'de> for Tile {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Tile, D::Error> {
        let [x, y] = <[i32; 2]>::deserialize(d)?;
        Ok(Tile { x, y })
    }
}

/// What the simulation needs to know about one kind of tile. Mirrors the
/// `tiles` object of `static/world/atlas.schema.json` field for field, so a
/// map can be validated against the atlas it draws with.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct TileDef {
    /// Atlas tile key: `floor/wood`, `wall/plaster`, `object/workbench`.
    pub key: String,
    /// Pixels of vertical extent above the floor plane, straight from the
    /// atlas. Converted to world z by [`crate::map::height`].
    #[serde(default)]
    pub height: u16,
    #[serde(default)]
    pub occludes: bool,
    #[serde(default = "unit_footprint")]
    pub footprint: [u8; 2],
    #[serde(default = "yes")]
    pub walkable: bool,
}

fn unit_footprint() -> [u8; 2] {
    [1, 1]
}

const fn yes() -> bool {
    true
}

impl TileDef {
    /// The tile layer the key belongs to, taken from its prefix.
    pub fn layer(&self) -> Layer {
        match self.key.split('/').next().unwrap_or("") {
            "wall" => Layer::Wall,
            "object" => Layer::Object,
            _ => Layer::Floor,
        }
    }
}

/// The three layers a `MapDef` stores per tile.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Layer {
    Floor,
    Wall,
    Object,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn distances() {
        let a = Tile::new(0, 0);
        let b = Tile::new(3, 4);
        assert_eq!(a.chebyshev(b), 4);
        assert_eq!(a.dist2(b), 25);
        assert_eq!(b.chebyshev(a), 4);
    }

    #[test]
    fn tile_json_is_a_pair() {
        let json = serde_json::to_string(&Tile::new(-2, 7)).unwrap();
        assert_eq!(json, "[-2,7]");
        assert_eq!(
            serde_json::from_str::<Tile>("[-2,7]").unwrap(),
            Tile::new(-2, 7)
        );
    }

    #[test]
    fn tile_def_defaults_match_the_atlas_schema() {
        let d: TileDef = serde_json::from_str(r#"{"key":"floor/wood"}"#).unwrap();
        assert_eq!(d.footprint, [1, 1]);
        assert!(d.walkable);
        assert!(!d.occludes);
        assert_eq!(d.height, 0);
        assert_eq!(d.layer(), Layer::Floor);
    }

    #[test]
    fn layer_comes_from_the_key_prefix() {
        let mk = |k: &str| TileDef {
            key: k.into(),
            height: 0,
            occludes: false,
            footprint: [1, 1],
            walkable: true,
        };
        assert_eq!(mk("wall/brick").layer(), Layer::Wall);
        assert_eq!(mk("object/table").layer(), Layer::Object);
        assert_eq!(mk("floor/moss").layer(), Layer::Floor);
        assert_eq!(mk("nonsense").layer(), Layer::Floor);
    }
}
