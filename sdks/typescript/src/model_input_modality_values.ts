/** Content modalities accepted by one configured model. */
export const ModelInputModality = Object.freeze({
  Text: "text",
  Image: "image",
  Audio: "audio",
  Video: "video",
  textOnly(): readonly ModelInputModality[] {
    return Object.freeze([this.Text]);
  },
  allSupported(): readonly ModelInputModality[] {
    return Object.freeze([this.Text, this.Image, this.Audio, this.Video]);
  },
} as const);

export type ModelInputModality = "text" | "image" | "audio" | "video";
