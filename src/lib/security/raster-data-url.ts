const rasterImageMediaTypes = new Set(["image/png", "image/jpeg", "image/gif", "image/webp"]);
const rasterDataUrlPattern = /^data:image\/(png|jpeg|gif|webp);base64,[A-Za-z0-9+/]+={0,2}$/u;
const maxRasterDataUrlLength = 2_800_000;

export function isRasterImageMediaType(value: unknown): value is string {
  return typeof value === "string" && rasterImageMediaTypes.has(value);
}

export function isSafeRasterDataUrl(value: unknown, expectedMediaType?: string): value is string {
  return (
    typeof value === "string" &&
    value.length <= maxRasterDataUrlLength &&
    rasterDataUrlPattern.test(value) &&
    (expectedMediaType === undefined || value.startsWith(`data:${expectedMediaType};base64,`))
  );
}
