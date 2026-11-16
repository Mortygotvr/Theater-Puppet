"""Theater Puppet's MediaPipe helper: a camera's face, body and hands, one
JSON line per frame on stdout, for the plugin (src/tracking/mediapipe.rs),
which starts it and stops it.

    python puppet_mediapipe.py --camera 0 --models <folder> [--face] [--pose] [--hands] [--parent <pid>]

Needs: pip install mediapipe opencv-python
The models are downloaded into --models the first time (about 20 MB).
It stops by itself when the program that started it (--parent) is gone.
"""
import argparse, json, os, sys, threading, time, urllib.request

MODELS = {
    "face": ("face_landmarker.task", "https://storage.googleapis.com/mediapipe-models/face_landmarker/face_landmarker/float16/1/face_landmarker.task"),
    "pose": ("pose_landmarker_full.task", "https://storage.googleapis.com/mediapipe-models/pose_landmarker/pose_landmarker_full/float16/1/pose_landmarker_full.task"),
    "hands": ("hand_landmarker.task", "https://storage.googleapis.com/mediapipe-models/hand_landmarker/hand_landmarker/float16/1/hand_landmarker.task"),
}


def say(**what):
    sys.stdout.write(json.dumps(what, separators=(",", ":")) + "\n")
    sys.stdout.flush()


def model(folder, kind):
    name, url = MODELS[kind]
    path = os.path.join(folder, name)
    if not os.path.isfile(path):
        say(status=f"downloading {name}")
        os.makedirs(folder, exist_ok=True)
        urllib.request.urlretrieve(url, path + ".part")
        os.replace(path + ".part", path)
    return path


class Latest:
    """DirectShow queues frames: read on a thread, keep only the newest."""

    def __init__(self, cap):
        self.cap, self.cond, self.frame, self.n, self.seen, self.ok = cap, threading.Condition(), None, 0, 0, True
        threading.Thread(target=self._loop, daemon=True).start()

    def _loop(self):
        while True:
            ok, im = self.cap.read()
            with self.cond:
                self.ok, self.frame, self.n = ok, im, self.n + 1
                self.cond.notify_all()
            if not ok:
                break

    def read(self):
        with self.cond:
            self.cond.wait_for(lambda: self.n != self.seen or not self.ok, timeout=3)
            fresh = self.n != self.seen
            self.seen = self.n
            return self.ok and fresh and self.frame is not None, self.frame


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--camera", type=int, default=0)
    ap.add_argument("--models", required=True)
    ap.add_argument("--face", action="store_true")
    ap.add_argument("--pose", action="store_true")
    ap.add_argument("--hands", action="store_true")
    ap.add_argument("--parent", type=int, default=0)
    a = ap.parse_args()

    # The plugin gone (OBS closed, or killed): so is this
    # (not by reading stdin: on Windows a waiting read holds up the imports)
    def watch():
        try:
            if os.name == "nt":
                import ctypes
                k = ctypes.windll.kernel32
                k.OpenProcess.restype = ctypes.c_void_p
                k.WaitForSingleObject.argtypes = [ctypes.c_void_p, ctypes.c_uint32]
                handle = k.OpenProcess(0x00100000, False, a.parent)  # SYNCHRONIZE
                if not handle:
                    return
                k.WaitForSingleObject(handle, 0xFFFFFFFF)
            else:
                while os.getppid() == a.parent:
                    time.sleep(1)
        except Exception:
            return
        os._exit(0)

    if a.parent:
        threading.Thread(target=watch, daemon=True).start()

    say(status="loading MediaPipe")
    try:
        import cv2
        import mediapipe as mp
        from mediapipe.tasks import python as tasks
        from mediapipe.tasks.python import vision
    except Exception as e:
        say(error=f"this Python hasn't mediapipe and opencv ({e}): pip install mediapipe opencv-python")
        return 2

    base = lambda kind: tasks.BaseOptions(model_asset_path=model(a.models, kind))
    video = vision.RunningMode.VIDEO
    face = pose = hands = None
    try:
        if a.face:
            face = vision.FaceLandmarker.create_from_options(vision.FaceLandmarkerOptions(
                base_options=base("face"), running_mode=video, num_faces=1,
                output_face_blendshapes=True, output_facial_transformation_matrixes=True))
        if a.pose:
            pose = vision.PoseLandmarker.create_from_options(vision.PoseLandmarkerOptions(base_options=base("pose"), running_mode=video, num_poses=1))
        if a.hands:
            hands = vision.HandLandmarker.create_from_options(vision.HandLandmarkerOptions(base_options=base("hands"), running_mode=video, num_hands=2))
    except Exception as e:
        say(error=f"MediaPipe's models didn't load: {e}")
        return 3

    say(status=f"opening camera {a.camera}")
    cap = cv2.VideoCapture(a.camera, cv2.CAP_DSHOW) if os.name == "nt" else cv2.VideoCapture(a.camera)
    if not cap.isOpened():
        say(error=f"camera {a.camera} didn't open (in use by another program, or by an OBS source?)")
        return 4
    cap.set(cv2.CAP_PROP_FOURCC, cv2.VideoWriter_fourcc(*"MJPG"))
    cap.set(cv2.CAP_PROP_FRAME_WIDTH, 640)
    cap.set(cv2.CAP_PROP_FRAME_HEIGHT, 480)
    cap.set(cv2.CAP_PROP_FPS, 30)
    say(status=f"camera {a.camera}: {int(cap.get(3))} x {int(cap.get(4))}")
    camera = Latest(cap)

    r3 = lambda v: round(float(v), 4)
    start, last, frames, shown = time.time(), 0, 0, time.time()
    while True:
        ok, im = camera.read()
        if not ok:
            if not camera.ok:
                say(error="the camera stopped")
                return 5
            continue
        ms = max(int((time.time() - start) * 1000), last + 1)
        last = ms
        image = mp.Image(image_format=mp.ImageFormat.SRGB, data=cv2.cvtColor(im, cv2.COLOR_BGR2RGB))
        out = {}
        if face:
            f = face.detect_for_video(image, ms)
            if f.face_blendshapes and f.facial_transformation_matrixes:
                out["face"] = {
                    "shapes": {c.category_name: r3(c.score) for c in f.face_blendshapes[0]},
                    # 4 x 4, row by row: the face in the camera's space (cm; X right in the picture, Y up, Z toward the camera)
                    "matrix": [r3(v) for v in f.facial_transformation_matrixes[0].reshape(-1)],
                }
        if pose:
            p = pose.detect_for_video(image, ms)
            if p.pose_world_landmarks:
                out["pose"] = {
                    # 33 x (metres from the hips' middle: X right in the picture, Y down, Z away; how visible)
                    "world": [[r3(l.x), r3(l.y), r3(l.z), r3(l.visibility or 0)] for l in p.pose_world_landmarks[0]],
                    "wrists": [[r3(p.pose_landmarks[0][i].x), r3(p.pose_landmarks[0][i].y)] for i in (15, 16)],
                }
        if hands:
            h = hands.detect_for_video(image, ms)
            if h.hand_world_landmarks:
                out["hands"] = [{
                    "world": [[r3(l.x), r3(l.y), r3(l.z)] for l in w],
                    "wrist": [r3(n[0].x), r3(n[0].y)],
                    "label": c[0].category_name,
                } for w, n, c in zip(h.hand_world_landmarks, h.hand_landmarks, h.handedness)]
        frames += 1
        if time.time() - shown > 5:
            out["fps"] = round(frames / (time.time() - shown), 1)
            frames, shown = 0, time.time()
        say(**out)


if __name__ == "__main__":
    sys.exit(main() or 0)
