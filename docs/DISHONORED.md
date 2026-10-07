# Dishonored mode

Press **K** in a match you host and Corvo takes over the local soldier:
Dishonored's walk, sprint, crouch, slide, mantle, lean, swim, ladders and
Blink, with his arms, sword, Blink effects and sounds. Press K again for the
soldier. Everything Dishonored is read at startup from your own install; none
of it ships here. The motion and the install readers are
[sinhonor](https://github.com/Nacholmo/sinhonor)'s crates, a git dependency
pinned by tag in `Cargo.toml`; its tests run there, against your install.

| | keyboard / mouse | controller |
|---|---|---|
| toggle | K, or `dishonored on\|off\|status` | both sticks in |
| move, look, jump, crouch, sprint | your MW2 binds | your MW2 layout |
| Blink: hold to aim, release to go | ADS (right mouse) or F | ADS trigger |
| Blink tier I / II | 1 / 2 | D-pad down / up |
| lean | Q / E | bumpers |
| slow walk | Alt | |

The install is found through `IW4L_DISHONORED`, then `DISHONORED_DIR`, then the
Steam libraries. `IW4L_DISHONORED_DIFFICULTY=easy|normal|hard|veryhard` picks
Corvo's attribute set. One Unreal unit is a centimetre, so Corvo keeps his
size: 1.75 m tall, 4 m/s running, a 2.1 m jump.

| piece | where |
|---|---|
| mode, input, effect and sound triggers | `crates/render_anim/src/dishonored/mod.rs` |
| sweeps on PMove's collision, unit and axis conversion | `…/dishonored/world.rs` |
| arms and sword, Edge animations, CPU skinning | `…/dishonored/hands.rs` |
| Cascade particles as sprite batches | `…/dishonored/fx.rs` |
| Wwise cues decoded to PCM, footstep pacing | `…/dishonored/sound.rs` |
| arms, sprites and Blink vignette on the GPU | `crates/render_gpu/src/drawsurf/dishonored.rs` |
| PMove handed to an outside controller | `sim::SimWorld::{set_external_motion, with_player_clip}` |
| decoded clips as 2D one-shots | `audio::AudioRuntime::play_external` |
| camera, hidden weapon and reticle, zeroed commands | `view_kick.rs`, `fpv_present.rs`, `reticle.rs`, `net/src/client/runtime.rs` |

Like the skate mode it follows, it moves the player on the local authority, so
a client joined to someone else's match cannot turn it on. Other players see
the soldier's body glide without leg animation, Dishonored's fall damage is
not applied, and Blink's camera motion blur is only the vignette.
