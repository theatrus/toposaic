"use client";

import { useEffect, useRef, useState } from "react";
import {
  Box3,
  BufferGeometry,
  DirectionalLight,
  Float32BufferAttribute,
  HemisphereLight,
  Mesh,
  MeshStandardMaterial,
  PerspectiveCamera,
  Scene,
  Vector3,
  WebGLRenderer,
} from "three";
import { OrbitControls } from "three/examples/jsm/controls/OrbitControls.js";
import type { GlobePreviewMesh } from "./contracts";

const PART_COLORS = { tile: "#8ba589", frame: "#526c73", joint: "#a78058", clip: "#d5a258" };

export function GlobePreview({ meshes, status, error, onCancel }: {
  meshes?: GlobePreviewMesh[];
  status: string;
  error?: string | null;
  onCancel?: () => void;
}) {
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const [view, setView] = useState("assembled");
  const [rendererError, setRendererError] = useState(false);
  const visibleView = view.startsWith("tile-") && !meshes?.some((mesh) => mesh.name === view)
    ? "assembled"
    : view;

  useEffect(() => {
    const canvas = canvasRef.current;
    if (!canvas || !meshes?.length) return;
    let renderer: WebGLRenderer;
    try {
      renderer = new WebGLRenderer({ canvas, alpha: true, antialias: true });
    } catch {
      const frame = requestAnimationFrame(() => setRendererError(true));
      return () => cancelAnimationFrame(frame);
    }
    renderer.setPixelRatio(Math.min(window.devicePixelRatio, 2));
    const scene = new Scene();
    const camera = new PerspectiveCamera(38, 1, 0.1, 3000);
    const controls = new OrbitControls(camera, canvas);
    controls.enablePan = false;
    const key = new DirectionalLight(0xfff2db, 2.25);
    key.position.set(4, 6, 5);
    scene.add(key, new HemisphereLight(0xf3f7ff, 0x5e675d, 1.85));
    const bounds = new Box3();
    const rendered: Mesh[] = [];
    for (const part of meshes) {
      if (visibleView === "frame" && part.kind === "tile") continue;
      if (visibleView.startsWith("tile-") && part.name !== visibleView) continue;
      part.poses.forEach((pose, index) => {
        const spread = visibleView === "exploded" && part.kind === "tile" ? 32 : 0;
        const positions = part.vertices.flatMap(([x, y, z]) => {
          const point = pose.origin.map((origin, axis) => origin + x * pose.x_axis[axis]
            + y * pose.y_axis[axis] + z * pose.z_axis[axis] + spread * part.outward[index][axis]);
          return [point[0], point[2], -point[1]];
        });
        const geometry = new BufferGeometry();
        geometry.setAttribute("position", new Float32BufferAttribute(positions, 3));
        geometry.setIndex(part.triangles.flat());
        geometry.computeVertexNormals();
        geometry.computeBoundingBox();
        if (geometry.boundingBox) bounds.union(geometry.boundingBox);
        const material = new MeshStandardMaterial({ color: PART_COLORS[part.kind], roughness: 0.85, flatShading: true });
        const mesh = new Mesh(geometry, material);
        rendered.push(mesh);
        scene.add(mesh);
      });
    }
    const center = bounds.getCenter(new Vector3());
    const point = new Vector3();
    let radiusSquared = 0;
    for (const mesh of rendered) {
      const positions = mesh.geometry.getAttribute("position");
      for (let index = 0; index < positions.count; index += 1) {
        point.fromBufferAttribute(positions, index);
        radiusSquared = Math.max(radiusSquared, point.distanceToSquared(center));
      }
    }
    const radius = Math.sqrt(radiusSquared);
    controls.target.copy(center);
    controls.minDistance = radius * 0.35;
    controls.maxDistance = radius * 12;
    const direction = new Vector3(1.35, 0.9, 1.45).normalize();
    let initialSize = true;
    const draw = () => renderer.render(scene, camera);
    const resize = () => {
      const width = Math.max(1, canvas.clientWidth);
      const height = Math.max(1, canvas.clientHeight);
      camera.aspect = width / height;
      camera.updateProjectionMatrix();
      setRendererError(false);
      if (initialSize) {
        initialSize = false;
        camera.position.copy(center).addScaledVector(direction, radius * 3.35 / Math.min(camera.aspect, 1));
        controls.maxDistance = Math.max(controls.maxDistance, camera.position.distanceTo(center) * 1.1);
        controls.update();
      }
      renderer.setSize(width, height, false);
      draw();
    };
    const observer = new ResizeObserver(resize);
    observer.observe(canvas);
    controls.addEventListener("change", draw);
    const onKeyDown = (event: KeyboardEvent) => {
      switch (event.key) {
        case "ArrowLeft": controls.rotateLeft(0.12); break;
        case "ArrowRight": controls.rotateLeft(-0.12); break;
        case "ArrowUp": controls.rotateUp(0.12); break;
        case "ArrowDown": controls.rotateUp(-0.12); break;
        case "+":
        case "=": controls.dollyOut(1.1); break;
        case "-": controls.dollyIn(1.1); break;
        default: return;
      }
      event.preventDefault();
      controls.update();
    };
    canvas.addEventListener("keydown", onKeyDown);
    resize();
    return () => {
      observer.disconnect();
      canvas.removeEventListener("keydown", onKeyDown);
      controls.dispose();
      for (const mesh of rendered) {
        mesh.geometry.dispose();
        (mesh.material as MeshStandardMaterial).dispose();
      }
      renderer.dispose();
    };
  }, [meshes, visibleView]);

  return (
    <section className="relief-shell globe-preview" aria-label="3D globe preview">
      <canvas ref={canvasRef} className="relief-canvas" data-globe-parts={meshes?.length ?? 0} aria-label="Globe assembly" tabIndex={0} />
      <div className="globe-preview-toolbar">
        <label>
          View
          <select aria-label="Assembly view" value={visibleView} onChange={(event) => setView(event.target.value)} disabled={!meshes?.length}>
            <option value="assembled">Assembled</option>
            <option value="frame">Frame only</option>
            <option value="exploded">Exploded</option>
            {meshes?.filter((mesh) => mesh.kind === "tile").map((mesh) => (
              <option key={mesh.name} value={mesh.name}>Tile {mesh.name.slice(5)}</option>
            ))}
          </select>
        </label>
        {onCancel && <button type="button" onClick={onCancel}>Cancel preview</button>}
        <span role="status">{error ?? (rendererError ? "3D preview requires WebGL." : status)}</span>
      </div>
    </section>
  );
}
