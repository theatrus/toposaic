export function previewWorldX(eastwardPosition: number) {
  return 0.5 - eastwardPosition;
}

export function previewInitialCameraPosition(
  cameraScale: number,
  targetHeight: number,
): [number, number, number] {
  return [
    0.92 * cameraScale,
    targetHeight + 0.72 * cameraScale,
    -1.08 * cameraScale,
  ];
}
