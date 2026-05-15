"use strict";
(() => {
    function isRecord(value) {
        return typeof value === "object" && value !== null && !Array.isArray(value);
    }
    function extractTextPayload(input) {
        return input?.result?.content?.[0]?.text ?? null;
    }
    function parsePayload(input) {
        if (isRecord(input) && isRecord(input.data)) {
            return input;
        }
        if (isRecord(input) && Array.isArray(input.results)) {
            return input;
        }
        const textPayload = extractTextPayload(input);
        if (typeof textPayload === "string" && textPayload.length > 0) {
            try {
                return JSON.parse(textPayload);
            }
            catch {
                return { raw: textPayload };
            }
        }
        return input;
    }
    function normalizeExtractionEnvelope(input) {
        const payload = parsePayload(input);
        if (isRecord(payload) && Array.isArray(payload.results)) {
            const rows = [];
            const errors = [];
            for (const entry of payload.results) {
                if (isRecord(entry) && isRecord(entry.data)) {
                    rows.push(entry.data);
                }
                if (isRecord(entry) && typeof entry.error === "string") {
                    errors.push(entry.error);
                }
            }
            return {
                mode: "batch",
                data: null,
                rows,
                metadata: {
                    total_matched: payload.total_matched ?? rows.length,
                    successful: payload.successful ?? rows.length,
                    root_selector: payload.root_selector ?? null,
                },
                debug: payload.debug ?? null,
                errors,
                rawPayload: payload,
            };
        }
        if (isRecord(payload) && isRecord(payload.data)) {
            const metadata = isRecord(payload.metadata) ? payload.metadata : {};
            const rawErrors = metadata.errors;
            const errors = Array.isArray(rawErrors)
                ? rawErrors.filter((item) => typeof item === "string")
                : [];
            return {
                mode: "single",
                data: payload.data,
                rows: [payload.data],
                metadata,
                debug: payload.debug ?? null,
                errors,
                rawPayload: payload,
            };
        }
        return {
            mode: "unknown",
            data: null,
            rows: [],
            metadata: {},
            debug: null,
            errors: [],
            rawPayload: payload,
        };
    }
    function escapeCsvCell(value) {
        const text = value == null ? "" : String(value);
        if (/[",\n]/.test(text)) {
            return `"${text.replace(/"/g, '""')}"`;
        }
        return text;
    }
    function recordsToCsv(records) {
        if (records.length === 0) {
            return "";
        }
        const headers = Array.from(records.reduce((acc, record) => {
            Object.keys(record).forEach((key) => acc.add(key));
            return acc;
        }, new Set()));
        const lines = [headers.map(escapeCsvCell).join(",")];
        for (const record of records) {
            const row = headers.map((header) => escapeCsvCell(record[header]));
            lines.push(row.join(","));
        }
        return lines.join("\n");
    }
    globalThis.StygianResultUtils = {
        normalizeExtractionEnvelope,
        recordsToCsv,
    };
})();
//# sourceMappingURL=result-utils.js.map