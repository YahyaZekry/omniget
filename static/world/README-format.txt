OmniGet world art format (v1) - atlas.json + PNG pages
Normative schema: atlas.schema.json. Tools: src-tauri/omniget-world-tools.

WHY IT LOOKS LIKE THIS
  A character faces 8 ways on screen but only 5 of them are drawn. SE, E and NE
  are the horizontal flip of SW, W and NW, so the art and the texture cost half
  as much. The atlas says so with "mirror_of"; drawing them by hand is refused.
  Every frame carries a pivot: the pixel where the feet touch the floor. The
  world sorts by y + z, so the pivot and the tile height are what make a
  character walk behind a wall instead of through it.

THE FILES
  atlas.json     frames (rects), anims (fps + 8 directions), tiles (height)
  <sheet>-N.png  the pages, at most 2048x2048, plain sRGB, no colour profile

PACKING (atlas-pack <src>... <out-dir>)
  Character source:  <src>/<sheet>/<anim>/<DIR>/<n>.png, DIR in S SW W NW N,
                     frames numbered from 0. Optional <src>/manifest.json:
                     { "default_fps": 10,
                       "anims": { "omni/walk": { "fps": 12, "loop": true,
                                                 "pivot": [24,62], "z_base": 0 } } }
                     No manifest: 10 fps and pivot = bottom centre, [w/2, h-1].
  Tile source:       <src>/{floor,wall,object}/<name>.png plus <src>/tiles.json:
                     { "tiles": { "wall/brick": { "height": 32, "occludes": true,
                                                  "footprint": [1,1],
                                                  "walkable": false } } }
                     The folder name loses a trailing "-src" to name the sheet.
  Frames get 2 px of padding and 1 px of edge extrusion (the border pixel is
  repeated outside the rect, so linear filtering cannot pull in the neighbour).
  The output is deterministic: same input, same bytes, no timestamps.
  Try it with no art: atlas-pack --fixture <dir> writes a synthetic source tree.

CHECKING (atlas-check <atlas.json>...)
  Exit 1 with one ERR_ATLAS_* line per problem. It refuses: a pivot moving more
  than 1 px inside an animation (the character bobs); SE/E/NE with their own
  sheet or mirroring the wrong direction; a page over 2048 px; a frame outside
  its page; a page carrying iCCP/cHRM/gAMA; a frame id or tile frame that does
  not exist. Run it on every atlas before it reaches the renderer.
