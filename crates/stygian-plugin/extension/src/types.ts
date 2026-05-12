/**
 * Type definitions for the Stygian Plugin extension
 */

/**
 * A region to extract from a template
 */
export interface Region {
  name: string;
  selector: Selector;
  schema: Record<string, any>;
  transformations: Transformation[];
}

/**
 * CSS and/or XPath selector with fallback support
 */
export interface Selector {
  type: "css" | "xpath" | "dual";
  css?: string;
  xpath?: string;
}

/**
 * Transformation to apply to extracted values
 */
export type Transformation =
  | { type: "Trim" }
  | { type: "Lowercase" }
  | { type: "Uppercase" }
  | { type: "RemoveWhitespace" }
  | { type: "NormalizeWhitespace" }
  | { type: "StripHtml" }
  | { type: "DecodeHtml" }
  | { type: "ParseJson" }
  | { type: "Regex"; pattern: string; replacement: string }
  | { type: "RegexExtract"; pattern: string; group: number }
  | { type: "Filter"; pattern: string };

/**
 * Template metadata
 */
export interface TemplateMetadata {
  created_at: string;
  updated_at: string;
  last_used_at?: string;
  usage_count: number;
  version: number;
  tags: string[];
}

/**
 * Extraction template
 */
export interface ExtractionTemplate {
  id: string;
  name: string;
  description?: string;
  regions: Region[];
  metadata: TemplateMetadata;
}

/**
 * Extraction request
 */
export interface ExtractionRequest {
  template_id: string;
  url: string;
  html: string;
  idempotency_key?: string;
}

/**
 * Extraction result
 */
export interface ExtractionResult {
  data: Record<string, any>;
  metadata: {
    regions_successful: number;
    total_regions: number;
    elapsed_ms: number;
  };
}

/**
 * Recording state for template capture
 */
export interface RecordingState {
  active: boolean;
  template_name: string;
  current_region?: {
    name: string;
    selector: string;
  };
  regions: Region[];
}

/**
 * Message types for extension communication
 */
export interface Message {
  type: string;
  [key: string]: any;
}

export interface CreateTemplateMessage extends Message {
  type: "create_template";
  name: string;
  description?: string;
}

export interface AddRegionMessage extends Message {
  type: "add_region";
  region: Region;
}

export interface ApplyTemplateMessage extends Message {
  type: "apply_template";
  template_id: string;
}

export interface ListTemplatesMessage extends Message {
  type: "list_templates";
}

export interface GetTemplateMessage extends Message {
  type: "get_template";
  template_id: string;
}

export interface DeleteTemplateMessage extends Message {
  type: "delete_template";
  template_id: string;
}

export interface StartRecordingMessage extends Message {
  type: "start_recording";
  template_name: string;
}

export interface StopRecordingMessage extends Message {
  type: "stop_recording";
}

/**
 * Response types from backend
 */
export interface BackendResponse<T> {
  success: boolean;
  data?: T;
  error?: string;
}

/**
 * DOM element with selector path
 */
export interface ElementWithPath {
  element: Element;
  css_path: string;
  xpath_path: string;
}
