"""Deterministic handcrafted yard layout and generated sprite preparation."""
import json
from PIL import Image, ImageOps

# Sprite size, ground pivot, collision footprint, height. Additive atlas keys.
PROPS = {
    'crop-bed': ((144, 100), (56, 58), (3, 2), 12),
    'apple-tree': ((112, 146), (56, 136), (1, 1), 64),
    'fence': ((106, 78), (21, 43), (3, 1), 24),
    'shed': ((190, 200), (95, 158), (3, 3), 80),
    'flower-clump': ((48, 40), (24, 32), (1, 1), 8),
}


def prepare(raw, out):
    meta = json.loads((out / 'tiles.json').read_text())
    for name, (size, pivot, footprint, height) in PROPS.items():
        source = Image.open(raw / 'farm' / f'{name}.png').convert('RGBA')
        # Exclude the near-transparent generation halo from trim bounds.
        box = source.getchannel('A').point(lambda a: 255 if a >= 80 else 0).getbbox()
        sprite = ImageOps.contain(source.crop(box), size, Image.Resampling.LANCZOS)
        tile = Image.new('RGBA', size)
        tile.alpha_composite(sprite, ((size[0]-sprite.width)//2, size[1]-sprite.height))
        tile.save(out / 'object' / f'{name}.png')
        meta['tiles'][f'object/{name}'] = dict(pivot=pivot, footprint=footprint,
            height=height, occludes=height >= 64, walkable=False)
    (out / 'tiles.json').write_text(json.dumps(meta, indent=2)+'\n')
    return meta['tiles']


def generate(house, tiles):
    palette = list(house['palette'])
    for name in PROPS:
        key = 'object/'+name
        palette.append(dict(key=key, **{k: tiles[key][k] for k in ('footprint','height','occludes','walkable')}))
    keys = {p['key']: i for i,p in enumerate(palette)}
    chunks=[]
    for cx in range(2):
        floor=[]
        for y in range(32,48):
            for x in range(cx*16,cx*16+16):
                inside = 2<=x<=29 and 33<=y<=46
                # Clip the corners of the felt lawn instead of a rigid rectangle.
                inside = inside and not ((x<4 or x>27) and y>44)
                path = (8<=x<=10) or (y in (38,39) and 5<=x<=26) or (x in (20,21) and 39<=y<=45)
                # A workshop terrace in front of the tool shed.
                terrace = 3<=x<=7 and 34<=y<=39
                key = 'floor/cobble' if path or terrace else 'floor/moss'
                floor.append(keys[key] if inside or (x==9 and y==32) else 255)
        chunks.append(dict(cx=cx,cy=2,floor=floor,wall=[255]*256,object=[255]*256,height=[0]*256))
    m=dict(version=1,id='yard-v1',atlas='world/tiles/craft-v1/atlas.json',chunk_tiles=16,
        palette=palette,chunks=chunks,slots=[],objects=[],rooms=[
            dict(id='yard-entry',rect=[8,32,3,8]),dict(id='yard-work',rect=[2,33,6,7]),
            dict(id='yard-garden',rect=[11,33,10,14]),dict(id='yard-orchard',rect=[22,33,8,14])],
        markers=[dict(name='house-door',tile=[9,32]),dict(name='spawn',tile=[9,33]),
            dict(name='approval',tile=[10,36]),dict(name='visitor',tile=[10,35]),
            dict(name='loop-garden',tile=[20,42]),dict(name='delivery',tile=[7,41])])
    def place(oid,name,key,x,y,approach=False):
        m['slots'].append(dict(id=name,kind='floor',tile=[x,y],accepts=[key],default=key,dir=0))
        m['objects'].append(dict(id=oid,kind=key,tile=[x,y],slot=name,dir=0))
        if approach:
            m['markers'].append(dict(name=name,tile=[x,y+tiles[key].get('footprint',[1,1])[1]]))
    # Four functional stations: two at the workshop, two alongside the beds.
    for i,(x,y) in enumerate(((4,38),(12,37),(17,37),(23,38)),1):
        place(100+i,f'yard-work-{i}','object/workbench',x,y,True)
    for oid,name,key,x,y in [(105,'yard-rest','chair',6,43),(106,'yard-delivery','chest',7,40),
                            (107,'yard-knowledge','bookshelf',3,37)]:
        place(oid,name,'object/'+key,x,y,True)
    decorations=[('shed',3,33)]
    decorations += [('crop-bed',x,y) for y in (34,41,44) for x in (12,16)]
    decorations += [('apple-tree',x,y) for x,y in ((23,34),(27,35),(24,42),(28,43),(3,42))]
    decorations += [('fence',x,y) for x,y in ((12,33),(16,33),(22,33),(26,33),(12,46),(16,46),(23,46))]
    decorations += [('flower-clump',x,y) for x,y in ((7,34),(11,33),(20,34),(21,36),(28,38),
        (11,40),(19,40),(22,40),(4,45),(7,45),(11,46),(20,46),(27,45),(27,40),(2,39))]
    for i,(key,x,y) in enumerate(decorations,200):
        place(i,f'farm-{key}-{i}','object/'+key,x,y)
    return m
