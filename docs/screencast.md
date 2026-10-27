# Recording the README GIFs

The pictures in `README.md` are GIFs in `media/`, up to 1000 px wide. Record
them on a display of your own, never on the user's: your synthesized keys land
on whatever window has the focus, and the user may be working. A private
display also gives the same window size and the same result every time.

You need `Xvfb`, `openbox`, `ffmpeg` and `python-xlib`. The driver scripts are
not kept with the tool; write them as you need them.

```
Xvfb :77 -screen 0 1520x1030x24 -nolisten tcp &
DISPLAY=:77 openbox &
env -u WAYLAND_DISPLAY DISPLAY=:77 WINIT_X11_SCALE_FACTOR=1 ./target/release/tilepicky &
```

Four things that each cost an hour to find out:

- **Unset `WAYLAND_DISPLAY`.** winit picks Wayland whenever that variable is
  set, whatever `DISPLAY` says, and the window opens on the user's screen
  instead of yours. `WINIT_UNIX_BACKEND` no longer overrides this.
- **`WINIT_X11_SCALE_FACTOR=1`** makes the app draw one pixel per point. On a
  scaled display it draws at 1.8, and the text is unreadable once the capture
  is scaled down to 1000 px.
- **`ffmpeg -f x11grab` reads the root window.** On the user's Wayland
  session that gives a black rectangle, because XWayland windows are
  redirected. On Xvfb nothing composites, so it works.
- **openbox draws the title bar**, which carries the name and the version.
  Its frame has no name of its own: find it as the parent of the app window,
  not by searching for the title.

Frame it narrow. A window wider than its content leaves half the picture
empty, and the README shows the picture about 880 px wide, so everything in
it shrinks. Around 900 by 930 fits the two panels with little waste. Zoom a
sheet until it fills its panel, and show a different pack in each picture:
two captures of the same tiles are one capture.

Decide the take before you film it. Say what a reader should think when
they see it, and build a project that makes them think it: the tool picks
tiles out of a pack into a tilesheet of your own, so the film pulls a whole
house out of a packed sheet and plants two trees beside it. Paste replaces
pixels, it does not blend them, so a transparent sprite dropped on a filled
tile leaves a hole; a sheet that fills up reads as work, a map with holes in
it reads as a broken tilesheet. Caption the keys: a film of a keyboard with
no keys in it looks the same as a film of a mouse.

Drive the app with XTEST through python-xlib. Stop the instance you started
last: the driver takes the first window it finds that is named Tilepicky, so
a stray one takes every key while the screenshot shows the window on top of
it. Prepare the state off camera and record only the part worth watching. Do
not count the rows of a tree: the keys arrive on the file that is open, else
on the first row, so a count from the top opens the wrong file as soon as a
file is open. Walk to the end, where the cursor stops, and step back from
there.

```
DISPLAY=:77 ffmpeg -y -f x11grab -draw_mouse 0 -framerate 15 \
  -video_size <w>x<h> -i :77.0+<x>,<y> -t 22 demo.mp4
ffmpeg -i demo.mp4 -vf "fps=10,scale=1000:-1:flags=lanczos,split[a][b];\
[a]palettegen=max_colors=128[p];[b][p]paletteuse=dither=bayer:bayer_scale=3" \
  -loop 0 media/<name>.gif
```

Point the project folder at something tidy first. `mytilesheets/` holds a
small, well named set for this; `mytiles/` is a scratch folder and its names
show in every frame.
