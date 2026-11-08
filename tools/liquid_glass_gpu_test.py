#!/usr/bin/env python3
"""Headless readback regression tests using the production WGSL, not a shader copy.

Install: python -m pip install wgpu==0.32.0
Run: python tools/liquid_glass_gpu_test.py
Use --shader PATH --record to collect a pre-fix baseline without assertions.
Optional --output DIR writes raw JSON and PPM diagnostic images.
A Vulkan software adapter is sufficient; no desktop/window is required.
"""
import argparse
import json
from pathlib import Path
import struct

import wgpu

ROOT = Path(__file__).resolve().parents[1]
SIZE = 16


def decode(c):
    return c / 12.92 if c <= 0.04045 else ((c + 0.055) / 1.055) ** 2.4


def encode(c):
    return c * 12.92 if c <= 0.0031308 else 1.055 * c ** (1 / 2.4) - 0.055


def material(rgb):
    """Independent CPU oracle for the flat interior, tint=0, saturation=1."""
    y = sum(a * b for a, b in zip(rgb, (0.299, 0.587, 0.114)))
    t = min(1, max(0, (y - 0.15) / 0.7))
    t = t * t * (3 - 2 * t)
    adaptive = [a + (b - a) * t for a, b in zip((0.82, 0.90, 1), (1, 0.98, 0.94))]
    return [c * 0.45 + (c * a + a * 0.045) * 0.55 for c, a in zip(rgb, adaptive)]


class Harness:
    def __init__(self, shader):
        self.adapter = wgpu.gpu.request_adapter_sync(power_preference="low-power")
        self.device = self.adapter.request_device_sync()
        self.shader = self.device.create_shader_module(code=shader)
        stages = wgpu.ShaderStage.FRAGMENT
        entries = [dict(binding=0, visibility=stages, buffer=dict(type="uniform"))]
        for b in (1, 3, 4, 5):
            entries.append(dict(binding=b, visibility=stages, texture=dict(sample_type="float", view_dimension="2d")))
        entries.extend([
            dict(binding=2, visibility=stages, sampler=dict(type="filtering")),
            dict(binding=6, visibility=stages, buffer=dict(type="read-only-storage")),
            dict(binding=7, visibility=stages, buffer=dict(type="uniform")),
        ])
        self.layout = self.device.create_bind_group_layout(entries=entries)
        self.pipeline_layout = self.device.create_pipeline_layout(bind_group_layouts=[self.layout])
        self.sampler = self.device.create_sampler(mag_filter="nearest", min_filter="nearest")

    def texture(self, pixels, fmt="rgba8unorm"):
        texture = self.device.create_texture(size=(SIZE, SIZE, 1), format=fmt,
            usage=wgpu.TextureUsage.TEXTURE_BINDING | wgpu.TextureUsage.COPY_DST)
        if len(pixels) == 4:
            pixels = pixels * (SIZE * SIZE)
        data = struct.pack("<" + "e" * len(pixels), *pixels) if fmt == "rgba16float" else bytes(round(max(0,min(1,x)) * 255) for x in pixels)
        stride = SIZE * (8 if fmt == "rgba16float" else 4)
        self.device.queue.write_texture(dict(texture=texture), data,
            dict(bytes_per_row=stride, rows_per_image=SIZE), (SIZE, SIZE, 1))
        return texture

    def render(self, rgb, *, replacement=1, coverage=1, presentation=1, flags=0,
               height=1, blur_rgb=None, entry="fs_main", scene_format="rgba8unorm", scene_alpha=1, clear=(0,0,0,0)):
        u = bytearray(160)
        for offset, values in {0:(SIZE,SIZE),12:(40,),16:(1.5,),24:(6,),28:(1,),48:(-0.6,-0.8),68:(1,),88:(replacement,),104:(SIZE,SIZE),116:(presentation,)}.items():
            struct.pack_into("<" + "f" * len(values),u,offset,*values)
        struct.pack_into("<I", u, 72, 1)
        struct.pack_into("<I", u, 76, flags)
        uniform = self.device.create_buffer_with_data(data=u,usage=wgpu.BufferUsage.UNIFORM)
        stats = self.device.create_buffer_with_data(data=bytes(96),usage=wgpu.BufferUsage.STORAGE)
        auto = self.device.create_buffer_with_data(data=bytes(16),usage=wgpu.BufferUsage.UNIFORM)
        backdrop = self.texture([*rgb,scene_alpha],scene_format)
        blur = self.texture([*(blur_rgb or rgb),1])
        geometry = self.texture([0.5,0.5,height,coverage],"rgba16float")
        tint = self.texture([0,0,0,0],"rgba16float")
        bindings = [dict(binding=0,resource=dict(buffer=uniform)),dict(binding=2,resource=self.sampler),
            dict(binding=6,resource=dict(buffer=stats)),dict(binding=7,resource=dict(buffer=auto))]
        for b,t in ((1,backdrop),(3,geometry),(4,blur),(5,tint)):
            bindings.append(dict(binding=b,resource=t.create_view()))
        group = self.device.create_bind_group(layout=self.layout,entries=bindings)
        blend = {k:dict(src_factor="one",dst_factor="one-minus-src-alpha",operation="add") for k in ("color","alpha")}
        pipeline = self.device.create_render_pipeline(layout=self.pipeline_layout,
            vertex=dict(module=self.shader,entry_point="vs_main"),
            fragment=dict(module=self.shader,entry_point=entry,targets=[dict(format="rgba8unorm-srgb",blend=blend)]))
        target = self.device.create_texture(size=(SIZE,SIZE,1),format="rgba8unorm-srgb",
            usage=wgpu.TextureUsage.RENDER_ATTACHMENT | wgpu.TextureUsage.COPY_SRC)
        encoder = self.device.create_command_encoder()
        render = encoder.begin_render_pass(color_attachments=[dict(view=target.create_view(),resolve_target=None,
            clear_value=clear,load_op="clear",store_op="store")])
        render.set_pipeline(pipeline)
        render.set_bind_group(0,group)
        render.draw(3)
        render.end()
        self.device.queue.submit([encoder.finish()])
        data = bytes(self.device.queue.read_texture(dict(texture=target),dict(bytes_per_row=SIZE*4,rows_per_image=SIZE),(SIZE,SIZE,1)))
        return list(data[:4]), data

    def flatten(self, scene_rgba, desktop_rgb, *, scene_format="rgba8unorm-srgb", shader_path=None):
        """Exercise the completed-scene -> encoded capture boundary."""
        path = shader_path or ROOT / "assets/shaders/liquid_glass_scene_flatten.wgsl"
        shader = self.device.create_shader_module(code=path.read_text())
        scene = self.texture(scene_rgba, scene_format)
        desktop = self.texture([*desktop_rgb, 1])
        uniform = self.device.create_buffer_with_data(
            data=struct.pack("<8f", SIZE, SIZE, 0, 0, SIZE, SIZE, 0, 0), usage=wgpu.BufferUsage.UNIFORM)
        pipeline = self.device.create_render_pipeline(layout="auto",
            vertex=dict(module=shader, entry_point="vs_main"),
            fragment=dict(module=shader, entry_point="fs_main", targets=[dict(format="rgba8unorm")]))
        group = self.device.create_bind_group(layout=pipeline.get_bind_group_layout(0), entries=[
            dict(binding=0, resource=scene.create_view()), dict(binding=1, resource=desktop.create_view()),
            dict(binding=2, resource=self.sampler), dict(binding=3, resource=dict(buffer=uniform))])
        target = self.device.create_texture(size=(SIZE, SIZE, 1), format="rgba8unorm",
            usage=wgpu.TextureUsage.RENDER_ATTACHMENT | wgpu.TextureUsage.COPY_SRC)
        encoder = self.device.create_command_encoder()
        render = encoder.begin_render_pass(color_attachments=[dict(view=target.create_view(), resolve_target=None,
            clear_value=(0, 0, 0, 0), load_op="clear", store_op="store")])
        render.set_pipeline(pipeline)
        render.set_bind_group(0, group)
        render.draw(3)
        render.end()
        self.device.queue.submit([encoder.finish()])
        return list(bytes(self.device.queue.read_texture(dict(texture=target),
            dict(bytes_per_row=SIZE * 4, rows_per_image=SIZE), (SIZE, SIZE, 1)))[:4])

    def present(self, scene_rgba, mode, fmt):
        shader = self.device.create_shader_module(code=(ROOT / "src/shader_surface_present.wgsl").read_text())
        scene = self.texture(scene_rgba, fmt)
        uniform = self.device.create_buffer_with_data(data=struct.pack("<4I", mode, int(fmt.endswith("srgb")), 0, 0),
            usage=wgpu.BufferUsage.UNIFORM)
        pipeline = self.device.create_render_pipeline(layout="auto",
            vertex=dict(module=shader, entry_point="vs_main"),
            fragment=dict(module=shader, entry_point="fs_main", targets=[dict(format=fmt)]))
        group = self.device.create_bind_group(layout=pipeline.get_bind_group_layout(0), entries=[
            dict(binding=0, resource=scene.create_view()), dict(binding=1, resource=dict(buffer=uniform))])
        target = self.device.create_texture(size=(SIZE, SIZE, 1), format=fmt,
            usage=wgpu.TextureUsage.RENDER_ATTACHMENT | wgpu.TextureUsage.COPY_SRC)
        encoder = self.device.create_command_encoder()
        render = encoder.begin_render_pass(color_attachments=[dict(view=target.create_view(), resolve_target=None,
            clear_value=(0, 0, 0, 0), load_op="clear", store_op="store")])
        render.set_pipeline(pipeline)
        render.set_bind_group(0, group)
        render.draw(3)
        render.end()
        self.device.queue.submit([encoder.finish()])
        return list(bytes(self.device.queue.read_texture(dict(texture=target),
            dict(bytes_per_row=SIZE * 4, rows_per_image=SIZE), (SIZE, SIZE, 1)))[:4])


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--shader",type=Path,default=ROOT/"assets/shaders/liquid_glass_final.wgsl")
    parser.add_argument("--record",action="store_true")
    parser.add_argument("--flatten-shader",type=Path,default=ROOT/"assets/shaders/liquid_glass_scene_flatten.wgsl")
    parser.add_argument("--output",type=Path)
    parser.add_argument("--skip-presentation",action="store_true",help="For pre-change --record only")
    parser.add_argument("--legacy-scene-unorm",action="store_true",help="Record the old flatten view format; diagnostic only")
    args=parser.parse_args()
    if args.skip_presentation and not args.record:
        parser.error("--skip-presentation requires --record")
    if args.legacy_scene_unorm and not args.record:
        parser.error("--legacy-scene-unorm requires --record")
    h=Harness(args.shader.read_text())
    results={"adapter":dict(h.adapter.info),"cases":{}}
    cases=[]
    for value in (0,16,64,128,192,255):
        rgb=[value/255]*3
        cases.append((f"gray_{value}",rgb,{},material(rgb)))
    for name,rgb in (("red",[1,0,0]),("green",[0,1,0]),("blue",[0,0,1]),("mixed",[32/255,128/255,224/255])):
        cases.append((name,rgb,{},material(rgb)))
    cases.extend([
        ("capture_debug",[128/255]*3,{"flags":1},[128/255]*3),
        ("fallback_translucent",[128/255]*3,{"replacement":0},material([128/255]*3)),
        ("fallback_half_coverage",[128/255]*3,{"replacement":0,"coverage":0.5},material([128/255]*3)),
        ("replacement_sharp_rejected",[1,1,1],{"blur_rgb":[0,0,0]},material([0,0,0])),
        ("replacement_over_white",[128/255]*3,{"clear":(1,1,1,1)},material([128/255]*3)),
        ("replacement_over_red",[128/255]*3,{"clear":(1,0,0,1)},material([128/255]*3)),
        ("outside",[0.5]*3,{"coverage":0},[0,0,0]),
        ("aa_half",[128/255]*3,{"coverage":0.5},material([128/255]*3)),
        ("rim_aa_quarter",[128/255]*3,{"coverage":0.25,"height":0.5,"flags":64},material([128/255]*3)),
        ("presentation_zero",[128/255]*3,{"presentation":0},material([128/255]*3)),
        ("presentation_half",[128/255]*3,{"presentation":0.5},material([128/255]*3)),
        ("toggle_desktop_fallback",[0,0,0],{"entry":"fs_toggle","scene_format":"rgba8unorm-srgb","scene_alpha":0,"blur_rgb":[128/255]*3},[128/255]*3),
        ("toggle_partial_scene",[92/255]*3,{"entry":"fs_toggle","scene_format":"rgba8unorm-srgb","scene_alpha":128/255,"blur_rgb":[128/255]*3},[128/255]*3),
        ("toggle_clear",[128/255]*3,{"entry":"fs_toggle","scene_format":"rgba8unorm-srgb"},[128/255]*3),
    ])
    failures=[]
    if args.output: args.output.mkdir(parents=True,exist_ok=True)
    for name,rgb,kwargs,expected_rgb in cases:
        pixel,data=h.render(rgb,**kwargs)
        coverage=kwargs.get("coverage",1)
        replacement=kwargs.get("replacement",1)
        alpha=(coverage*replacement + min(coverage*0.64,0.92)*(1-replacement))*kwargs.get("presentation",1)
        expected=[min(255, round(encode(decode(c)*alpha)*255)) for c in expected_rgb]+[round(alpha*255)]
        results["cases"][name]={"actual":pixel,"expected":expected}
        if any(abs(a-b)>2 for a,b in zip(pixel,expected)):
            failures.append(name)
        if args.output:
            (args.output/f"{name}.ppm").write_bytes(f"P6\n{SIZE} {SIZE}\n255\n".encode()+b"".join(data[i:i+3] for i in range(0,len(data),4)))
    for name,scene,desktop in (
        ("flatten_empty",[0,0,0,0],[128/255]*3),
        ("flatten_opaque",[128/255]*3+[1],[1,0,0]),
        ("flatten_half",[92/255]*3+[128/255],[128/255]*3),
        ("flatten_color",[64/255,92/255,128/255,128/255],[224/255,32/255,64/255]),
    ):
        actual=h.flatten(scene,desktop,shader_path=args.flatten_shader,
            scene_format="rgba8unorm" if args.legacy_scene_unorm else "rgba8unorm-srgb")
        expected=[round(min(1,encode(decode(c)+decode(d)*(1-scene[3])))*255)
                  for c,d in zip(scene[:3],desktop)]+[255]
        results["cases"][name]={"actual":actual,"expected":expected}
        if any(abs(a-b)>2 for a,b in zip(actual,expected)): failures.append(name)
        # Feed the actual completed capture into another panel's production
        # shader, as menu/settings lanes do after flattening and blurring.
        nested_rgb=[c/255 for c in actual[:3]]
        nested,_=h.render(nested_rgb,clear=(1,0,0,1))
        nested_expected=[round(min(1,c)*255) for c in material(nested_rgb)]+[255]
        results["cases"][name+"_nested"]={"actual":nested,"expected":nested_expected}
        if any(abs(a-b)>2 for a,b in zip(nested,nested_expected)): failures.append(name+"_nested")
    if not args.skip_presentation:
        for fmt in ("rgba8unorm", "rgba8unorm-srgb"):
            for mode in (1, 2):
                for alpha in (0, 0.25, 0.5, 1):
                    for color_name, rgb in (("gray", [128/255]*3), ("color", [32/255,128/255,224/255])):
                        # Use the same stored scene convention as the renderer.
                        linear = [decode(c)*alpha for c in rgb]
                        stored = [encode(c) if fmt.endswith("srgb") else c for c in linear]
                        scene = [round(c*255)/255 for c in stored] + [round(alpha*255)/255]
                        actual = h.present(scene, mode, fmt)
                        sampled = [decode(c) if fmt.endswith("srgb") else c for c in scene[:3]]
                        expected_rgb = [0,0,0] if not scene[3] else [encode(c/scene[3]) for c in sampled]
                        if mode == 1: expected_rgb = [c*scene[3] for c in expected_rgb]
                        expected = [round(min(1,c)*255) for c in expected_rgb]+[round(scene[3]*255)]
                        name=f"present_{fmt}_{mode}_{alpha}_{color_name}"
                        results["cases"][name]={"actual":actual,"expected":expected}
                        if any(abs(a-b)>2 for a,b in zip(actual,expected)): failures.append(name)
        # End-to-end half-covered glass -> encoded-premultiplied OS surface.
        source,_=h.render([128/255]*3,coverage=0.5)
        actual=h.present([c/255 for c in source],1,"rgba8unorm-srgb")
        expected=[round(c*128) for c in material([128/255]*3)]+[128]
        results["cases"]["glass_to_os_half"]={"actual":actual,"expected":expected}
        if any(abs(a-b)>2 for a,b in zip(actual,expected)): failures.append("glass_to_os_half")
    print(json.dumps(results,indent=2))
    if args.output: (args.output/"results.json").write_text(json.dumps(results,indent=2)+"\n")
    if failures and not args.record: raise SystemExit("FAIL: "+", ".join(failures))
    print("RECORDED" if args.record else f"PASS: {len(results['cases'])} GPU readback cases")


if __name__ == "__main__":
    main()
