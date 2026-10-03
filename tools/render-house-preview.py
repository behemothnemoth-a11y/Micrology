"""Render exported engine quads for static inspection, not GPU/game footage.
Usage: python tools/render-house-preview.py NEW_EXPORT_DIRECTORY
Requires numpy and Pillow. No network or engine-world modification.
"""
from __future__ import annotations
import argparse
import hashlib
import json
from pathlib import Path
import numpy as np
from PIL import Image, ImageDraw, ImageFont

W, H = 1600, 1120
PALETTE = {101: (159,113,67), 102: (161,72,53), 103: (157,160,163), 104: (103,184,199)}

def font(size: int) -> ImageFont.FreeTypeFont | ImageFont.ImageFont:
    for name in ('C:/Windows/Fonts/segoeui.ttf', '/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf'):
        try:
            return ImageFont.truetype(name, size)
        except OSError:
            pass
    return ImageFont.load_default()

def render(root: Path, name: str, full: list[dict]) -> dict:
    source = root / f'{name}-surfaces.json'
    records = json.loads(source.read_text(encoding='utf-8'))
    d = np.array([1.25, 1.05, -1.7]); d /= np.linalg.norm(d)
    r = np.cross(np.array([0.,1.,0.]), d); r /= np.linalg.norm(r)
    u = np.cross(d,r)
    basis = np.stack([r,u,d],axis=1)
    all_points = np.array([p for q in full for p in q['corners']]) @ basis
    lo,hi = all_points[:,:2].min(axis=0), all_points[:,:2].max(axis=0)
    scale = min(1320/(hi[0]-lo[0]), 780/(hi[1]-lo[1]))
    center = (lo+hi)/2
    def screen(points: np.ndarray) -> np.ndarray:
        p = np.asarray(points,dtype=float) @ basis
        p[:,0] = (p[:,0]-center[0])*scale+W/2
        p[:,1] = -(p[:,1]-center[1])*scale+550
        return p
    pixels=np.empty((H,W,3),dtype=np.uint8);pixels[:]=(232,237,239)
    depth=np.full((H,W),-np.inf)
    light=np.array([-0.4,0.85,-0.3]);light/=np.linalg.norm(light)
    visible=0
    for q in records:
        corners=np.asarray(q['corners'],dtype=float)
        normal=np.cross(corners[1]-corners[0],corners[2]-corners[0]);normal/=np.linalg.norm(normal)
        if np.dot(normal,d)<=0: continue
        visible+=1
        shade=0.55+0.45*max(0.,float(np.dot(normal,light)))
        color=np.asarray(PALETTE[q['material']])*shade
        pts=screen(corners)
        for ids in [(0,1,2),(0,2,3)]:
            a,b,c=pts[list(ids)]
            xmin=max(0,int(np.floor(min(a[0],b[0],c[0]))));xmax=min(W-1,int(np.ceil(max(a[0],b[0],c[0]))))
            ymin=max(0,int(np.floor(min(a[1],b[1],c[1]))));ymax=min(H-1,int(np.ceil(max(a[1],b[1],c[1]))))
            if xmin>xmax or ymin>ymax:continue
            yy,xx=np.mgrid[ymin:ymax+1,xmin:xmax+1];xx=xx+0.5;yy=yy+0.5
            denom=(b[1]-c[1])*(a[0]-c[0])+(c[0]-b[0])*(a[1]-c[1])
            if abs(denom)<1e-10:continue
            wa=((b[1]-c[1])*(xx-c[0])+(c[0]-b[0])*(yy-c[1]))/denom
            wb=((c[1]-a[1])*(xx-c[0])+(a[0]-c[0])*(yy-c[1]))/denom
            wc=1-wa-wb;z=wa*a[2]+wb*b[2]+wc*c[2]
            tile=depth[ymin:ymax+1,xmin:xmax+1]
            mask=(wa>=-1e-8)&(wb>=-1e-8)&(wc>=-1e-8)&(z>tile)
            tile[mask]=z[mask];pixels[ymin:ymax+1,xmin:xmax+1][mask]=color
    image=Image.fromarray(pixels);draw=ImageDraw.Draw(image)
    draw.rectangle((0,0,W,137),fill=(21,30,39))
    title='REFERENCE HOUSE / '+('INTACT' if name=='intact' else 'INSPECTION CUTAWAY')
    draw.text((54,29),title,font=font(38),fill='white')
    draw.text((56,87),'Actual Micrology cell surfaces  |  Static software preview  |  No destruction simulated',font=font(23),fill=(180,205,214))
    draw.rectangle((0,955,W,H),fill=(248,250,250))
    labels=['Wood: frame / roof','Masonry: sides / chimney','Concrete: slab / steps','Glass: opaque voxel proxy']
    for i,((mid,col),label) in enumerate(zip(PALETTE.items(),labels)):
        x=55+i*380;draw.rectangle((x,984,x+24,1008),fill=col)
        draw.text((x+34,981),label,font=font(20),fill=(25,40,49))
    draw.text((55,1030),'6.0 x 5.0 m wall footprint  |  2.5 m wall band  |  50 mm per cell (fixture annotation only)',font=font(24),fill=(25,40,49))
    draw.text((55,1071),'Glazing is one 50 mm cell thick, NOT a resolved real pane. Cutaway is a view, not damage.',font=font(20),fill=(70,86,92))
    bar=int(20*scale);x,y=70,910
    draw.line((x,y,x+bar,y),fill=(21,30,39),width=4)
    for xx in [x,x+bar]:draw.line((xx,y-7,xx,y+7),fill=(21,30,39),width=3)
    draw.text((x,y-36),'1 m orthographic scale',font=font(19),fill=(21,30,39))
    # A two-metre upright is annotation only: it is not part of cell occupancy.
    marker=screen(np.array([[144,0,18],[144,40,18]]))
    for p in marker:draw.line((p[0]-9,p[1],p[0]+9,p[1]),fill=(21,30,39),width=3)
    draw.line(tuple(marker[:,:2].ravel()),fill=(21,30,39),width=3)
    draw.text((marker[1,0]+12,marker[1,1]),'2 m',font=font(21),fill=(21,30,39))
    target=root/f'{name}-preview.png'
    if target.exists():raise FileExistsError(f'refusing to overwrite {target}')
    image.save(target)
    return {'source':source.name,'source_sha256':hashlib.sha256(source.read_bytes()).hexdigest(),'image':target.name,'image_sha256':hashlib.sha256(target.read_bytes()).hexdigest(),'compiled_quads':len(records),'visible_quads':visible,'projection':'orthographic, identical for intact and cutaway','kind':'software render of exported GreedyCompiler quads; not sandbox/GPU capture'}

def main() -> None:
    parser=argparse.ArgumentParser(description=__doc__);parser.add_argument('directory',type=Path);args=parser.parse_args()
    root=args.directory
    for name in ('intact-preview.png','cutaway-preview.png','preview-provenance.json'):
        if (root/name).exists():raise FileExistsError(f'refusing to overwrite {root/name}')
    full=json.loads((root/'intact-surfaces.json').read_text(encoding='utf-8'))
    report=[render(root,name,full) for name in ('intact','cutaway')]
    with (root/'preview-provenance.json').open('x',encoding='utf-8') as f:json.dump(report,f,indent=2)
    print(json.dumps(report,indent=2))
if __name__=='__main__':main()
