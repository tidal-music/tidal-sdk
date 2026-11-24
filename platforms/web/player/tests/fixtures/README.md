Clear browser media fixtures.
`tone.mpd` and its fragmented MP4 segments contain three seconds of generated
440 Hz AAC-LC audio, produced with:

```
ffmpeg -f lavfi -i sine=frequency=440:duration=3 -c:a aac -b:a 48k \
  -f dash -seg_duration 1 tone.mpd
```

`color.mp4` contains 2.2 seconds of generated 96×64 colour frames, recorded using
Chrome canvas.captureStream(10) and MediaRecorder(video/mp4, 24 kbit/s).
The tests use real Shaka and browser decoding. These fixtures establish clear
media behaviour; they do not establish catalogue availability, DRM playback or
audible quality.
