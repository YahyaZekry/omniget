//! Placed objects. The player moves and swaps them inside slots; the
//! simulation only cares where they are and whether they can be stood on,
//! sat on or worked at.

use crate::ents::id::ObjectId;
use crate::map::Tile;

/// An object in the world. `kind` is the atlas key, kept as a string because
/// the catalogue grows without the simulation being rebuilt; it is interned in
/// the string table before it goes on the wire, so it costs one varint there.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Object {
    pub id: ObjectId,
    pub kind: String,
    pub tile: Tile,
    pub dir: u8,
    /// Slot it occupies, if any.
    pub slot: Option<String>,
    /// Footprint in tiles, copied from the palette at placement time.
    pub footprint: [u8; 2],
    pub walkable: bool,
    /// Atlas pixel height, for depth sorting on the other side.
    pub height: u16,
}

impl Object {
    /// Tiles this object covers.
    pub fn covers(&self, t: Tile) -> bool {
        let w = self.footprint[0].max(1) as i32;
        let d = self.footprint[1].max(1) as i32;
        t.x >= self.tile.x && t.x < self.tile.x + w && t.y >= self.tile.y && t.y < self.tile.y + d
    }

    /// Where an agent stands to use this object: the tile just south of its
    /// footprint, which is the side the camera faces.
    pub fn approach(&self) -> Tile {
        Tile::new(self.tile.x, self.tile.y + self.footprint[1].max(1) as i32)
    }

    /// Objects an agent can sit on.
    pub fn is_seat(&self) -> bool {
        matches!(kind_leaf(&self.kind), "chair" | "bed" | "sofa" | "stool")
    }

    /// Objects an agent can work at; `Decision::Work` refuses anything else.
    pub fn is_workstation(&self) -> bool {
        matches!(
            kind_leaf(&self.kind),
            "workbench" | "desk" | "table" | "bookshelf"
        )
    }

    pub fn is_bed(&self) -> bool {
        kind_leaf(&self.kind) == "bed"
    }
}

/// `object/workbench` -> `workbench`.
pub fn kind_leaf(kind: &str) -> &str {
    kind.rsplit('/').next().unwrap_or(kind)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn obj(kind: &str, fp: [u8; 2]) -> Object {
        Object {
            id: ObjectId(1),
            kind: kind.into(),
            tile: Tile::new(4, 4),
            dir: 0,
            slot: None,
            footprint: fp,
            walkable: false,
            height: 20,
        }
    }

    #[test]
    fn footprint_decides_what_is_covered() {
        let o = obj("object/workbench", [2, 1]);
        assert!(o.covers(Tile::new(4, 4)));
        assert!(o.covers(Tile::new(5, 4)));
        assert!(!o.covers(Tile::new(6, 4)));
        assert!(!o.covers(Tile::new(4, 5)));
    }

    #[test]
    fn a_zero_footprint_still_covers_its_own_tile() {
        let o = obj("object/mug", [0, 0]);
        assert!(o.covers(Tile::new(4, 4)));
        assert_eq!(o.approach(), Tile::new(4, 5));
    }

    #[test]
    fn approach_is_south_of_the_footprint() {
        assert_eq!(obj("object/bed", [2, 2]).approach(), Tile::new(4, 6));
    }

    #[test]
    fn roles_come_from_the_key_leaf() {
        assert!(obj("object/chair", [1, 1]).is_seat());
        assert!(obj("object/bed", [2, 2]).is_seat());
        assert!(obj("object/bed", [2, 2]).is_bed());
        assert!(obj("object/workbench", [2, 1]).is_workstation());
        assert!(!obj("object/plant", [1, 1]).is_workstation());
        assert!(!obj("object/plant", [1, 1]).is_seat());
        assert_eq!(kind_leaf("workbench"), "workbench");
    }
}
