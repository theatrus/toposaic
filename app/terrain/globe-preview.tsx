"use client";

import { useEffect, useRef, useState } from "react";
import {
  Box3,
  BufferGeometry,
  Color,
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
import type { GlobePreviewMesh, PreviewData } from "./contracts";

const PART_COLORS = { tile: "#8ba589", frame: "#526c73", joint: "#a78058", clip: "#d5a258" };

export function GlobePreview({ meshes, palette, tiles, layoutSize, overview, source, exterior, surfaceSamples, status, error, onCancel }: {
  meshes?: GlobePreviewMesh[];
  palette?: Record<number, string>;
  tiles?: number[];
  layoutSize?: number;
  overview?: PreviewData["globe_overview"];
  source?: PreviewData["globe_source"];
  exterior?: PreviewData["globe_exterior"];
  surfaceSamples?: number;
  status: string;
  error?: string | null;
  onCancel?: () => void;
}) {
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const [view, setView] = useState("assembled");
  const [rendererError, setRendererError] = useState(false);
  const viewUnavailable = (view === "layout" && !overview)
    || (view.startsWith("tile-") && !meshes?.some((mesh) => mesh.name === view));
  const visibleView = viewUnavailable ? "assembled" : view;

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
    if (visibleView === "layout" && overview) {
      const highlights = new Set(tiles);
      const selected = new Color("#e2a346");
      const available = new Color("#8da49e");
      const positions: number[] = [];
      const colors: number[] = [];
      for (const [index, face] of overview.entries()) {
        const { r, g, b } = highlights.has(index + 1) ? selected : available;
        for (const [x, y, z] of face) {
          positions.push(x, z, -y);
          colors.push(r, g, b);
        }
      }
      const geometry = new BufferGeometry();
      geometry.setAttribute("position", new Float32BufferAttribute(positions, 3));
      geometry.setAttribute("color", new Float32BufferAttribute(colors, 3));
      geometry.computeVertexNormals();
      geometry.computeBoundingBox();
      if (geometry.boundingBox) bounds.union(geometry.boundingBox);
      const mesh = new Mesh(geometry, new MeshStandardMaterial({ vertexColors: true, flatShading: true }));
      rendered.push(mesh);
      scene.add(mesh);
    }
    for (const part of meshes) {
      if (visibleView === "layout") break;
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
        const painted = Boolean(part.materials?.length && palette);
        const topTriangles = part.kind === "tile" && exterior === "rounded" && surfaceSamples
          ? Math.min(surfaceSamples ** 2, part.triangles.length)
          : 0;
        const sourcePositions = new Float32BufferAttribute(positions, 3);
        let topGeometry: BufferGeometry | undefined;
        if (topTriangles) {
          // Smooth only the terrain surface. Tile seams, flat backs and snap
          // cavities should keep their own face normals.
          topGeometry = new BufferGeometry();
          topGeometry.setAttribute("position", sourcePositions);
          topGeometry.setIndex(part.triangles.slice(0, topTriangles).flat());
          topGeometry.computeVertexNormals();
        }
        if (painted || topGeometry) {
          const rgb = Object.fromEntries(Object.entries(palette ?? {}).map(([slot, hex]) => [slot, new Color(hex)]));
          const fallback = new Color(PART_COLORS[part.kind]);
          const flatPositions: number[] = [];
          const flatColors: number[] = [];
          for (const [i, triangle] of part.triangles.entries()) {
            const color = part.materials ? rgb[part.materials[i]] ?? fallback : fallback;
            for (const vertex of triangle) {
              flatPositions.push(positions[vertex * 3], positions[vertex * 3 + 1], positions[vertex * 3 + 2]);
              if (painted) flatColors.push(color.r, color.g, color.b);
            }
          }
          geometry.setAttribute("position", new Float32BufferAttribute(flatPositions, 3));
          if (painted) geometry.setAttribute("color", new Float32BufferAttribute(flatColors, 3));
        } else {
          geometry.setAttribute("position", sourcePositions);
          geometry.setIndex(part.triangles.flat());
        }
        geometry.computeVertexNormals();
        if (topGeometry) {
          const smooth = topGeometry.getAttribute("normal");
          const normals = geometry.getAttribute("normal");
          for (let i = 0; i < topTriangles; i += 1) {
            part.triangles[i].forEach((vertex, corner) => {
              normals.setXYZ(i * 3 + corner, smooth.getX(vertex), smooth.getY(vertex), smooth.getZ(vertex));
            });
          }
          topGeometry.dispose();
        }
        geometry.computeBoundingBox();
        if (geometry.boundingBox) bounds.union(geometry.boundingBox);
        const material = new MeshStandardMaterial({ color: painted ? 0xffffff : PART_COLORS[part.kind], vertexColors: painted, roughness: 0.85, flatShading: topTriangles === 0 });
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
    camera.near = Math.max(0.05, radius / 1000);
    camera.far = radius * 30;
    controls.target.copy(center);
    controls.minDistance = radius * 0.35;
    controls.maxDistance = radius * 12;
    const focus = meshes.find((part) => part.name === (visibleView.startsWith("tile-") ? visibleView : "tile-01"))
      ?? meshes.find((part) => part.kind === "tile");
    const [x, y, z] = focus?.outward[0] ?? [0.55, -0.55, 0.55];
    const direction = source === "whole_earth" && visibleView === "assembled" && tiles?.length === layoutSize
      ? new Vector3(1, 0.35, -0.44).normalize()
      : new Vector3(x + 0.25, z + 0.32, -y + 0.15).normalize();
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
  }, [exterior, layoutSize, meshes, overview, palette, source, surfaceSamples, tiles, visibleView]);

  return (
    <section className="relief-shell globe-preview" aria-label="3D globe preview">
      <canvas ref={canvasRef} className="relief-canvas" data-globe-parts={meshes?.length ?? 0} data-globe-samples={surfaceSamples} aria-label="Globe assembly" tabIndex={0} />
      <div className="globe-preview-toolbar">
        {tiles && layoutSize && (
          <span>{tiles.length === layoutSize ? `${layoutSize} tiles` : `${tiles.length} of ${layoutSize} tiles`}</span>
        )}
        <label>
          View
          <select aria-label="Assembly view" value={visibleView} onChange={(event) => setView(event.target.value)} disabled={!meshes?.length}>
            <option value="assembled">Assembled</option>
            {overview && <option value="layout">Whole globe layout</option>}
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
