// Match the two triangles used by the visible terrain mesh in each grid cell.
export function terrainSurfaceHeight(terrain, x, z, width, depth, minHeight, exaggeration) {
  const u = Math.max(0, Math.min(terrain.width - 1, (x / width + .5) * (terrain.width - 1)));
  const v = Math.max(0, Math.min(terrain.height - 1, (z / depth + .5) * (terrain.height - 1)));
  const col = Math.min(Math.floor(u), terrain.width - 2);
  const row = Math.min(Math.floor(v), terrain.height - 2);
  const a = u - col, b = v - row, w = terrain.width, values = terrain.values;
  const topLeft = values[row * w + col], topRight = values[row * w + col + 1];
  const bottomLeft = values[(row + 1) * w + col], bottomRight = values[(row + 1) * w + col + 1];
  const height = a + b <= 1
    ? topLeft * (1 - a - b) + topRight * a + bottomLeft * b
    : bottomRight * (a + b - 1) + bottomLeft * (1 - a) + topRight * (1 - b);
  return (height - minHeight) * width / 4940 * exaggeration;
}
