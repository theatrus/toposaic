export function elapsedLabel(milliseconds: number) {
  const safeMilliseconds = Math.max(0, milliseconds);
  if (safeMilliseconds < 1_000) {
    return `${Math.round(safeMilliseconds)} ms`;
  }
  const totalSeconds = Math.round(safeMilliseconds / 1_000);
  if (totalSeconds < 60) {
    return `${totalSeconds} s`;
  }
  const totalMinutes = Math.floor(totalSeconds / 60);
  const seconds = totalSeconds % 60;
  if (totalMinutes < 60) {
    return `${totalMinutes}m ${seconds}s`;
  }
  const hours = Math.floor(totalMinutes / 60);
  const minutes = totalMinutes % 60;
  return `${hours}h ${minutes}m`;
}
