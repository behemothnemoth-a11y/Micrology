"""Render admitted engine cell records, not animated/simulated falling debris.
Usage: python tools/render-house-impact-preview.py NEW_EXPORTED_DIRECTORY
The report is read-only. Existing PNG/provenance outputs are never overwritten.
"""
from __future__ import annotations
import argparse
import hashlib
import json
from pathlib import Path
from PIL import Image, ImageDraw, ImageFont

PALETTE={101:(159,113,67),102:(161,72,53),103:(157,160,163),104:(103,184,199)}
GROUP_COLORS=[(80,150,204),(229,140,74),(112,181,109),(184,119,190),(235,183,67),(86,190,183),(205,116,139)]

def font(size:int):
    for name in ('C:/Windows/Fonts/segoeui.ttf','/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf'):
        try:return ImageFont.truetype(name,size)
        except OSError:pass
    return ImageFont.load_default()

def main():
    parser=argparse.ArgumentParser(description=__doc__);parser.add_argument('directory',type=Path);args=parser.parse_args()
    source=args.directory/'report.json';output=args.directory/'window-impact-comparison.png';provenance=args.directory/'window-preview-provenance.json'
    if output.exists() or provenance.exists():raise FileExistsError('refusing to overwrite preview output')
    report=json.loads(source.read_text(encoding='utf-8'))
    width,height=1820,1060;image=Image.new('RGB',(width,height),(236,240,242));draw=ImageDraw.Draw(image)
    draw.rectangle((0,0,width,136),fill=(21,30,39))
    draw.text((45,23),'MICROLOGY / FIRST CONTROLLED HOUSE HIT',font=font(36),fill='white')
    draw.text((47,80),'Actual admitted cell records  |  One window pulse  |  No gravity or falling simulated',font=font(23),fill=(180,205,214))
    xlo,xhi,ylo,yhi=88,110,21,48;unit=19;top=217
    names=['BEFORE / INTACT WINDOW','AFTER / REMAINING STATIC','RELEASED / NATIVE GROUPS']
    captions=['Wood frame + opaque glass proxy','Released fragments hidden in this panel','Original positions; colors and numbers = IDs']
    panels=[report['window_before'],report['window_after_static'],[]]
    for i,piece in enumerate(report['pieces']):
        for c in piece['source_cells']:
            panels[2].append(dict(c,group=i))
    for i,records in enumerate(panels):
        left=48+i*590
        draw.text((left,157),names[i],font=font(26),fill=(21,30,39))
        draw.text((left,190),captions[i],font=font(18),fill=(65,79,91))
        nearest={}
        for c in records:
            x,y,z=c['pos']
            if not(xlo<=x<=xhi and ylo<=y<=yhi and 3<=z<=8):continue
            old=nearest.get((x,y))
            if old is None or z<old['pos'][2]:nearest[x,y]=c
        for x in range(xlo,xhi+1):
            for y in range(ylo,yhi+1):
                px=left+(x-xlo)*unit;py=top+(yhi-y)*unit
                c=nearest.get((x,y));fill=(252,253,254)
                if c:
                    fill=PALETTE[c['material']] if i<2 else GROUP_COLORS[c['group']%len(GROUP_COLORS)]
                draw.rectangle((px,py,px+unit,py+unit),fill=fill,outline=(211,220,225),width=1)
                if i==2 and c:
                    text=str(c['group']+1);box=draw.textbbox((0,0),text,font=font(10));tw=box[2]-box[0]
                    draw.text((px+(unit-tw)/2,py+2),text,font=font(10),fill=(16,26,37))
        if i<2:
            cx=left+(report['impact_center_cells'][0]-xlo)*unit
            cy=top+(yhi+1-report['impact_center_cells'][1])*unit
            radius=report['impact_radius_cells']*unit
            draw.ellipse((cx-radius,cy-radius,cx+radius,cy+radius),outline=(25,39,48),width=2)
            draw.line((cx-6,cy,cx+6,cy),fill=(20,28,38),width=2);draw.line((cx,cy-6,cx,cy+6),fill=(20,28,38),width=2)
        bottom=top+28*unit+18
        draw.line((left,bottom,left+4*unit,bottom),fill=(21,30,39),width=3)
        draw.text((left+4*unit+12,bottom-14),'20 cm / four cells',font=font(20),fill=(21,30,39))
    draw.rectangle((0,818,width,height),fill=(249,251,252))
    draw.text((46,839),f"{report['broken_bonds']} broken bonds   |   {report['fragment_cells']} cells released   |   {report['native_objects']} native fragments   |   {report['erased_cells']} cells erased",font=font(29),fill=(21,30,39))
    draw.text((46,884),'Piece sizes (cells): '+', '.join(map(str,report['sizes_cells'])),font=font(23),fill=(21,30,39))
    draw.text((46,925),'Grid = 5 cm per cell; glazing is a 5 cm proxy, NOT a resolved thin pane or fine glass shards.',font=font(23),fill=(57,74,83))
    draw.text((46,964),'Front slice only (z = 3..8 cells). Circle marks the 20 cm radius pulse, not the exact fracture boundary.',font=font(21),fill=(57,74,83))
    draw.text((46,1001),'No decorative particles, invented trajectories, or displaced fragments are shown. Abstract impact energy: '+str(report['abstract_energy']),font=font(21),fill=(57,74,83))
    with output.open('xb') as f:image.save(f,format='PNG')
    data={'source':source.name,'source_sha256':hashlib.sha256(source.read_bytes()).hexdigest(),'image':output.name,'image_sha256':hashlib.sha256(output.read_bytes()).hexdigest(),'kind':'orthographic cell-record diagnostic; not sandbox/GPU capture','front_slice_cell_bounds':[[88,21,3],[110,48,8]],'pixel_per_cell':unit,'hidden_in_middle_panel':'native fragments only; they still exist in the report','right_panel':'exact admitted fragment memberships in original source positions; no physics time step'}
    with provenance.open('x',encoding='utf-8',newline='\n') as f:json.dump(data,f,indent=2);f.write('\n')
    print(json.dumps(data,indent=2))
if __name__=='__main__':main()
