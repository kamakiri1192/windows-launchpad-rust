#!/usr/bin/env python3
"""GPU regression tests for the bottom bar's completed-scene backdrop.

Run after installing liquid_glass_gpu_requirements.txt. This executes the actual
flatten and final shaders; Rust tests separately verify which layers enter the
checkpoint. Use --image DIR for a diagnostic geometry/blur rendering (Pillow
required). These are synthetic shader fixtures, not application screenshots.
"""
import argparse
import json
from pathlib import Path
import struct

import wgpu
from liquid_glass_gpu_test import Harness, ROOT, decode, encode, material


def run_tests():
    h = Harness((ROOT / "assets/shaders/liquid_glass_final.wgsl").read_text())
    results = {"adapter": dict(h.adapter.info), "cases": {}}
    failures = []

    def check(name, actual, expected, tolerance=2):
        results["cases"][name] = {"actual": actual, "expected": expected}
        if any(abs(a-b) > tolerance for a,b in zip(actual,expected)):
            failures.append(name)

    desktop = [32/255, 64/255, 128/255]
    scenes = [
        ("empty", [0,0,0,0]),
        ("opaque_icon", [224/255,48/255,32/255,1]),
        ("drag_half", [round(encode(decode(c)*0.5)*255)/255 for c in (0.8,0.2,0.7)]+[128/255]),
        ("interaction_half", [round(encode(decode(c)*0.5)*255)/255 for c in (0.2,0.8,0.4)]+[128/255]),
    ]
    for name, scene in scenes:
        flattened = h.flatten(scene,desktop)
        expected = [round(min(1,encode(decode(c)+decode(d)*(1-scene[3])))*255)
                    for c,d in zip(scene[:3],desktop)]+[255]
        check(name+"_checkpoint",flattened,expected)
        capture = [c/255 for c in flattened[:3]]
        actual,_ = h.render(capture,replacement=1)
        check(name+"_bar",actual,[round(min(1,c)*255) for c in material(capture)]+[255])
        # A different existing target must not leak through a settled interior.
        other,_ = h.render(capture,replacement=1,clear=(1,0,1,1))
        check(name+"_lower_target_independent",other,actual,0)
        # Rendering again with the unchanged pre-bar checkpoint is stable.
        # This proves the shader contract, not the Rust checkpoint's ordering.
        again,_ = h.render(capture,replacement=1)
        check(name+"_repeat_checkpoint",again,actual,0)
        if name == "empty":
            main,_ = h.render(desktop,replacement=1)
            check("empty_matches_main_material",actual,main,0)
        else:
            wrong,_ = h.render(desktop,replacement=1)
            delta = max(abs(a-b) for a,b in zip(actual[:3],wrong[:3]))
            results["cases"][name+"_not_desktop_only"] = {"rgb_max_delta":delta,"minimum":10}
            if delta < 10: failures.append(name+"_not_desktop_only")

    for name,kwargs,alpha in (
        ("bar_edge_half",{"coverage":0.5},0.5),
        ("bar_fade_half",{"presentation":0.5},0.5),
        ("bar_fade_zero",{"presentation":0},0),
        ("bar_outside",{"coverage":0},0),
        ("bar_fallback",{"replacement":0},0.64),
        ("bar_fallback_edge",{"replacement":0,"coverage":0.5},0.32),
    ):
        actual,_ = h.render(desktop,**kwargs)
        expected=[round(min(1,encode(decode(c)*alpha))*255) for c in material(desktop)]+[round(alpha*255)]
        check(name,actual,expected)
    print(json.dumps(results,indent=2))
    if failures: raise SystemExit("FAIL: "+", ".join(failures))
    print(f"PASS: {len(results['cases'])} bottom-bar GPU regression cases")
    return results


def render_image(output):
    """Execute production geometry, flatten, blur, glass, and presentation."""
    from PIL import Image, ImageDraw, ImageFont
    import liquid_glass_gpu_test as base

    output.mkdir(parents=True,exist_ok=True)
    n=512
    base.SIZE=n
    h=Harness((ROOT/"assets/shaders/liquid_glass_final.wgsl").read_text())
    device=h.device
    usage=(wgpu.TextureUsage.TEXTURE_BINDING | wgpu.TextureUsage.RENDER_ATTACHMENT
           | wgpu.TextureUsage.COPY_SRC | wgpu.TextureUsage.COPY_DST)

    def tex(size,fmt):
        return device.create_texture(size=(size,size,1),format=fmt,usage=usage)

    def buf(data,kind=wgpu.BufferUsage.UNIFORM):
        return device.create_buffer_with_data(data=data,usage=kind)

    def shader(path):
        return device.create_shader_module(code=path.read_text())

    def entry(index,texture):
        return dict(binding=index,resource=texture.create_view())

    def write(texture,data):
        device.queue.write_texture(dict(texture=texture),data,
            dict(bytes_per_row=n*4,rows_per_image=n),(n,n,1))

    def draw(module,entries,targets,formats,*,layout="auto",load=False,blend=False):
        blend_state={k:dict(src_factor="one",dst_factor="one-minus-src-alpha",operation="add")
                     for k in ("color","alpha")}
        pipeline=device.create_render_pipeline(layout=layout,
            vertex=dict(module=module,entry_point="vs_main"),
            fragment=dict(module=module,entry_point="fs_main",targets=[
                dict(format=f,blend=blend_state if blend else None) for f in formats]))
        group=device.create_bind_group(layout=pipeline.get_bind_group_layout(0),entries=entries)
        encoder=device.create_command_encoder()
        render=encoder.begin_render_pass(color_attachments=[dict(view=t.create_view(),resolve_target=None,
            clear_value=(0,0,0,0),load_op="load" if load else "clear",store_op="store") for t in targets])
        render.set_pipeline(pipeline)
        render.set_bind_group(0,group)
        render.draw(3)
        render.end()
        device.queue.submit([encoder.finish()])

    def read(texture):
        return bytes(device.queue.read_texture(dict(texture=texture),
            dict(bytes_per_row=n*4,rows_per_image=n),(n,n,1)))

    def font(size):
        # Pillow ships a scalable fallback on platforms without DejaVu.
        try: return ImageFont.truetype("DejaVuSans.ttf",size)
        except OSError: return ImageFont.load_default(size=size)

    background=Image.new("RGBA",(n,n))
    pixels=background.load()
    for y in range(n):
        for x in range(n):
            pixels[x,y]=(int(35+140*x/n),int(45+110*y/n),int(150-70*x/n),255)
    painter=ImageDraw.Draw(background)
    for x in range(0,n,32): painter.line((x,0,x,n),fill=(190,194,210,255),width=1)
    for y in range(0,n,32): painter.line((0,y,n,y),fill=(190,194,210,255),width=1)
    painter.text((24,42),"CAPTURED DESKTOP",font=font(22),fill="white")
    for y in (160,300,440):
        painter.text((24,y),"Desktop text / fine detail",font=font(18),fill=(235,237,246,255))
    desktop=tex(n,"rgba8unorm")
    write(desktop,background.tobytes())

    # Synthetic lower UI checkpoint: two icons and a translucent dragged patch.
    # Their sRGB texture bytes encode linear-premultiplied scene RGB.
    scene_image=Image.new("RGBA",(n,n),(0,0,0,0))
    scene_draw=ImageDraw.Draw(scene_image)
    scene_draw.rounded_rectangle((54,257,160,381),radius=20,fill=(235,97,35,255))
    scene_draw.rounded_rectangle((204,289,308,405),radius=20,fill=(30,186,129,255))
    scene_draw.rounded_rectangle((355,247,462,371),radius=20,fill=(166,73,223,210))
    for x,y,label in ((82,276,"A"),(230,308,"B"),(380,265,"D")):
        scene_draw.text((x,y),label,font=font(44),fill=(255,255,255,255))
    scene_bytes=bytearray(scene_image.tobytes())
    for i in range(0,len(scene_bytes),4):
        alpha=scene_bytes[i+3]/255
        for c in range(3): scene_bytes[i+c]=round(encode(decode(scene_bytes[i+c]/255)*alpha)*255)
    lower=tex(n,"rgba8unorm-srgb")
    write(lower,scene_bytes)
    sampler=device.create_sampler(min_filter="linear",mag_filter="linear")
    flat_uniform=buf(struct.pack("<8f",n,n,0,0,n,n,0,0))
    complete=tex(n,"rgba8unorm")
    draw(shader(ROOT/"assets/shaders/liquid_glass_scene_flatten.wgsl"),[
        entry(0,lower),entry(1,desktop),dict(binding=2,resource=sampler),
        dict(binding=3,resource=dict(buffer=flat_uniform))],[complete],["rgba8unorm"])
    blur_uniform=buf(struct.pack("<4f",1,0,0,0))

    def blur(source):
        for filename,sizes in (("liquid_glass_blur_downsample.wgsl",(256,128,64)),
                               ("liquid_glass_blur_upsample.wgsl",(128,256,512))):
            module=shader(ROOT/"assets/shaders"/filename)
            for size in sizes:
                target=tex(size,"rgba8unorm")
                draw(module,[entry(0,source),dict(binding=1,resource=sampler),
                     dict(binding=2,resource=dict(buffer=blur_uniform))],[target],["rgba8unorm"])
                source=target
        return source

    desktop_blur=blur(desktop)
    complete_blur=blur(complete)
    uniforms=bytearray(160)
    for offset,values in {0:(n,n),12:(26,),16:(1.42,),20:(.075,),24:(16,),28:(1.34,),
                         32:(.94,.98,1,.045),48:(-.4472136,-.8944272),56:(1.25,),60:(.28,),
                         64:(26,),68:(260,),104:(n,n),116:(1,),128:(1,1,1,0),144:(0,0,n,n)}.items():
        struct.pack_into("<"+"f"*len(values),uniforms,offset,*values)
    struct.pack_into("<I",uniforms,72,1)
    shape=bytearray(96)
    struct.pack_into("<5fI",shape,0,256,360,420,76,38,2)
    struct.pack_into("<4f",shape,80,.94,.98,1,.045)
    geometry=tex(n,"rgba16float")
    tint=tex(n,"rgba8unorm")
    geometry_shader=device.create_shader_module(code=(ROOT/"src/shader_presentation.wgsl").read_text()+"\n"
        +(ROOT/"assets/shaders/liquid_glass_geometry.wgsl").read_text())
    draw(geometry_shader,[dict(binding=0,resource=dict(buffer=buf(uniforms))),
        dict(binding=1,resource=dict(buffer=buf(shape,wgpu.BufferUsage.STORAGE)))],
        [geometry,tint],["rgba16float","rgba8unorm"])
    pictures=[]
    for name,replacement,sharp,filtered in (("before",0,desktop,desktop_blur),
                                            ("after",1,complete,complete_blur)):
        struct.pack_into("<f",uniforms,88,replacement)
        scene=tex(n,"rgba8unorm-srgb")
        write(scene,scene_bytes)
        draw(h.shader,[dict(binding=0,resource=dict(buffer=buf(uniforms))),entry(1,sharp),
            dict(binding=2,resource=sampler),entry(3,geometry),entry(4,filtered),entry(5,tint),
            dict(binding=6,resource=dict(buffer=buf(bytes(96),wgpu.BufferUsage.STORAGE))),
            dict(binding=7,resource=dict(buffer=buf(bytes(16))))],
            [scene],["rgba8unorm-srgb"],layout=h.pipeline_layout,load=True,blend=True)
        presented=tex(n,"rgba8unorm-srgb")
        draw(shader(ROOT/"src/shader_surface_present.wgsl"),[entry(0,scene),
            dict(binding=1,resource=dict(buffer=buf(struct.pack("<4I",1,1,0,0))))],
            [presented],["rgba8unorm-srgb"])
        raw=read(presented)
        bg=background.tobytes()
        visible=bytearray(n*n*3)
        # Explicit SDR encoded-premultiplied OS source-over, after GPU readback.
        for i in range(n*n):
            alpha=raw[i*4+3]/255
            for c in range(3): visible[i*3+c]=min(255,round(raw[i*4+c]+bg[i*4+c]*(1-alpha)))
        image=Image.frombytes("RGB",(n,n),bytes(visible))
        image.save(output/f"bar-{name}.png")
        pictures.append(image)
    canvas=Image.new("RGB",(1072,662),(19,24,34))
    draw_text=ImageDraw.Draw(canvas)
    draw_text.text((24,15),"Bottom bar | actual WGSL headless render",font=font(23),fill="white")
    draw_text.text((24,53),"Before: desktop-only translucent bar",font=font(17),fill=(205,213,225))
    draw_text.text((560,53),"After: filtered completed lower scene",font=font(17),fill=(205,213,225))
    canvas.paste(pictures[0],(24,87))
    canvas.paste(pictures[1],(560,87))
    draw_text.text((24,613),"Same geometry / tint / optics. Synthetic icon + drag patches. Vulkan SwiftShader.",font=font(15),fill=(180,191,208))
    draw_text.text((24,638),"Shader diagnostic, not an app screenshot. OS source-over modeled after GPU readback.",font=font(13),fill=(180,191,208))
    path=output/"bar-before-after.png"
    canvas.save(path)
    print(path)


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output",type=Path)
    parser.add_argument("--image",type=Path)
    args=parser.parse_args()
    results=run_tests()
    if args.output:
        args.output.mkdir(parents=True,exist_ok=True)
        (args.output/"bar-results.json").write_text(json.dumps(results,indent=2)+"\n")
    if args.image:
        render_image(args.image)


if __name__ == "__main__":
    main()
