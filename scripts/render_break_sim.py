import json, math, os, sys
from pathlib import Path
from PIL import Image, ImageDraw, ImageFont

W, H = 1280, 720
SCALE = 8.0
OX, OY = 640.0, 520.0

BG = (12, 17, 24)
GRID = (28, 37, 49)
STATIC = (118, 142, 166)
ANCHOR = (66, 93, 120)
WEAK = (245, 206, 83)
DETACHED = (239, 122, 72)
REMOVED = (226, 74, 74)
TEXT = (235, 240, 246)
MUTED = (164, 177, 192)

def font(size, bold=False):
    candidates = [
        "/usr/share/fonts/truetype/dejavu/DejaVuSans-Bold.ttf" if bold else "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
        "/usr/share/fonts/truetype/liberation2/LiberationSans-Bold.ttf" if bold else "/usr/share/fonts/truetype/liberation2/LiberationSans-Regular.ttf",
    ]
    for p in candidates:
        if os.path.exists(p):
            return ImageFont.truetype(p, size)
    return ImageFont.load_default()

F_BIG = font(34, True)
F_MED = font(23, True)
F_SMALL = font(17, False)
F_MONO = font(15, False)

def project(p):
    x, y, z = p
    return (
        OX + (x - z) * SCALE,
        OY + (x + z) * SCALE * 0.43 - y * SCALE,
    )

def depth_key(p):
    x, y, z = p
    return (x + z, y, x)

def draw_cell(draw, p, color, radius=3):
    x, y = project(p)
    draw.rectangle((x-radius, y-radius, x+radius, y+radius), fill=color)

def base_canvas():
    img = Image.new("RGB", (W, H), BG)
    d = ImageDraw.Draw(img)
    # ground guide
    for x in range(0, 65, 8):
        a = project((x, 0, 0)); b = project((x, 0, 15))
        d.line((a, b), fill=GRID, width=1)
    for z in range(0, 16, 4):
        a = project((0, 0, z)); b = project((63, 0, z))
        d.line((a, b), fill=GRID, width=1)
    return img

def draw_static(img, cells, detached_preview=None, weak=None, removed=None):
    d = ImageDraw.Draw(img)
    preview = {tuple(p) for p in (detached_preview or [])}
    weakset = {tuple(p) for p in (weak or [])}
    for c in sorted(cells, key=lambda c: depth_key(c["p"])):
        p = tuple(c["p"])
        color = ANCHOR if c["anchored"] else STATIC
        if p in preview:
            color = DETACHED
        if p in weakset:
            color = WEAK
        draw_cell(d, p, color)
    for p in removed or []:
        x, y = project(p)
        d.ellipse((x-6, y-6, x+6, y+6), outline=REMOVED, width=2)

def qrot(v, q):
    x,y,z = v
    qx,qy,qz,qw = q
    # q * v * q^-1, optimized
    tx = 2*(qy*z - qz*y)
    ty = 2*(qz*x - qx*z)
    tz = 2*(qx*y - qy*x)
    return (
        x + qw*tx + (qy*tz - qz*ty),
        y + qw*ty + (qz*tx - qx*tz),
        z + qw*tz + (qx*ty - qy*tx),
    )

def draw_fragment(img, local_cells, translation, rotation):
    d = ImageDraw.Draw(img)
    pts=[]
    for p in local_cells:
        r=qrot((p[0]+0.5,p[1]+0.5,p[2]+0.5), rotation)
        w=(r[0]+translation[0], r[1]+translation[1], r[2]+translation[2])
        pts.append(w)
    for p in sorted(pts, key=depth_key):
        draw_cell(d,p,DETACHED,3)

def overlay(img, title, lines, badge=None):
    d=ImageDraw.Draw(img)
    d.rectangle((0,0,W,118), fill=(7,11,17))
    d.text((28,18), title, fill=TEXT, font=F_BIG)
    if badge:
        tw=d.textlength(badge,font=F_SMALL)
        d.rounded_rectangle((W-tw-56,24,W-28,55), 10, fill=(36,48,62))
        d.text((W-tw-42,30),badge,fill=TEXT,font=F_SMALL)
    y=68
    for line in lines:
        d.text((30,y),line,fill=MUTED,font=F_SMALL)
        y+=21

def save_stage(stage, report, path):
    img=base_canvas()
    draw_static(
        img, stage["static_cells"],
        stage.get("detached_preview",[]),
        stage.get("weak_cells",[]),
        stage.get("removed_now",[]),
    )
    overlay(
        img,
        stage["name"].replace("_"," ").title(),
        [
            f'weak plane x={report["weak_axis_x"]} • minimum complete cut={report["weak_cut_cells"]} cells',
            f'roots={stage["candidate_roots"]} • visited={stage["cells_visited"]} • detached components={stage["detached_components"]} • detached cells={stage["detached_cells"]}',
        ],
        "ACTUAL ENGINE STATE",
    )
    img.save(path)

def main():
    report_path=Path(sys.argv[1])
    outdir=Path(sys.argv[2])
    outdir.mkdir(parents=True,exist_ok=True)
    report=json.loads(report_path.read_text())
    frames=outdir/"frames"
    frames.mkdir(exist_ok=True)

    frame_no=0
    key_images=[]
    # Hold every discrete structural stage for 18 video frames.
    for idx,stage in enumerate(report["stages"]):
        key=outdir/f"stage_{idx:02d}.png"
        save_stage(stage,report,key)
        key_images.append(key)
        for _ in range(18):
            (frames/f"{frame_no:05d}.png").write_bytes(key.read_bytes())
            frame_no+=1

    # Static world after detach is last stage; animate the real Avian trace over it.
    static_stage=report["stages"][-1]
    frag=report["fragments"][0]
    for pf in report["physics_frames"]:
        img=base_canvas()
        draw_static(img, static_stage["static_cells"], [], [], [])
        draw_fragment(img,frag["local_cells"],pf["translation"],pf["rotation_xyzw"])
        overlay(
            img,
            "Avian Physics — Detached Fragment",
            [
                f't={pf["seconds"]:.2f}s • y={pf["translation"][1]:.3f} • sleeping={pf["sleeping"]}',
                f'fragment collider={frag["collision_boxes"]} merged boxes • storage={frag["storage_bytes"]} B • static colliders={report["static_collision_boxes_after_detach"]} boxes',
            ],
            "REAL AVIAN f64 TRACE",
        )
        img.save(frames/f"{frame_no:05d}.png")
        frame_no+=1

    # Final hold.
    last=frames/f"{frame_no-1:05d}.png"
    for _ in range(30):
        (frames/f"{frame_no:05d}.png").write_bytes(last.read_bytes())
        frame_no+=1

    # Contact sheet of meaningful structural moments.
    picks=[0, max(0,len(key_images)//2), len(key_images)-2, len(key_images)-1]
    thumbs=[]
    for i in picks:
        im=Image.open(key_images[i]).resize((600,338))
        thumbs.append(im)
    sheet=Image.new("RGB",(1200,676),BG)
    for j,im in enumerate(thumbs):
        sheet.paste(im,((j%2)*600,(j//2)*338))
    sheet.save(outdir/"weakpoint_break_contact_sheet.png")

if __name__=="__main__":
    main()
