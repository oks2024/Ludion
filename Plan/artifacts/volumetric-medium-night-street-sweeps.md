# night-street — volumetric-shaft sweeps

Decision sheets for "how do you get dramatic raking moonlight shafts in a vigil-shaped night
district". Rendered on `examples/gallery/night-street.yaml` (RTX 3090), a built-in-registry testbed
because `render --sweep` cannot reach a district's own components. Regenerate any sheet with the
command beside it.

The finding in one line: the engine's shafts already work; a district reads as a flat veil because
its steep follow-camera stares at the ground and crops out the air the shafts live in. The four
ingredients that produce beams — camera that sees the air, backlit azimuth, air-column occluders, a
dark backdrop — are baked into the committed scene; these sweeps isolate the three tunable ones.

## `-pitch.png` — the camera is the real blocker
```
ludion render --scene examples/gallery/night-street.yaml --width 1600 --height 900 \
  --sweep /entities/2/components/Transform/rotation/0=-12,-28,-44,-58
```
Same scene and lighting, camera pitch only. At −12° the shaft and shadow drama fill the frame; by
−44°/−58° (vigil's follow-cam angle) the camera looks almost straight down at dark ground and the
shafts — which live in the air above the street — are gone. This is why a district veils, and it is a
camera / game-side call, not a medium knob.

## `-moon.png` — moon intensity, the under-powered knob
```
ludion render --scene examples/gallery/night-street.yaml --width 1600 --height 900 \
  --sweep /entities/1/components/DirectionalLight/intensity=0.3,0.9,1.8,3.5
```
vigil authors the moon at 0.3 ("a deliberate whisper"). Raising it sharpens lit-vs-shadow contrast on
every surface AND brightens the beams, because the shaft is tinted by the moon's premultiplied colour
— two effects from one knob. Pair with a lower `ibl_intensity` to deepen the shadows underneath.

## `-shafts.png` — shafts alone is a brighter veil
```
ludion render --scene examples/gallery/night-street.yaml --width 1600 --height 900 \
  --sweep /entities/0/components/PostProcess/shafts=0.5,2.0,4.0,8.0
```
With occluders and a dark backdrop present (as in the committed scene), higher `shafts` reads as
beams up to a point; past ~4 the near air blows out. Without them — an open street or a steep camera
— the same sweep just fills the frame with milky haze, which is the "I don't see the volumetric
light" complaint. Raise it only once the camera frames the air.
