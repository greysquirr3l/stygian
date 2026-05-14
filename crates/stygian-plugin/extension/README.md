# Stygian Plugin - Chrome Extension

Interactive visual data extraction tool. Record extraction templates once, apply them anywhere. Fallback for when automated scraping fails.

## Scope and Positioning

This extension is a basic reference implementation for the Stygian plugin surface.
It is meant to demonstrate recording, template application, and MCP transport, not to
be a full production ingestion client.

For production usage, keep data routing and persistence policies on the server side:

- Extract in the extension (for list/table pages, prefer `plugin_extract_batch`)
- Send structured payloads to your MCP/ingestion backend
- Route records through graph services and sink adapters for durable storage

See the mdBook pattern guide for the end-to-end flow:
[Plugin Persistence Pattern](../../../book/src/mcp/plugin-persistence-pattern.md)

## Features

- **Visual Template Recording**: Click elements on a page to define extraction zones
- **Reusable Templates**: Save templates and apply them across multiple pages
- **Multi-Region Extraction**: Extract different types of data from different parts of a page
- **Transformation Pipeline**: Apply transformations (trim, regex, lowercase, etc.) to extracted data
- **Local Persistence**: Templates stored locally in Chrome storage (IndexedDB compatible)
- **Backend Sync**: Optional sync to Stygian backend for team collaboration

## Installation

### Development

1. Clone the stygian repository
2. Navigate to `crates/stygian-plugin/extension`
3. Install dependencies: `npm install` (when TypeScript setup is complete)
4. Compile TypeScript: `npx tsc`
5. Open Chrome and go to `chrome://extensions`
6. Enable "Developer mode" (top right)
7. Click "Load unpacked" and select the `dist/` directory

### Building

```bash
# From the extension directory
npm install
npm run build  # or: npx tsc
```

The compiled JavaScript will be in the `dist/` directory.

## Usage

### Creating a Template

1. Click the Stygian Plugin icon in your Chrome toolbar
2. Go to the "Record" tab
3. Enter a template name
4. Click "Start Recording"
5. Hover over elements on the page to preview them
6. Click on elements to add them as extraction regions
7. Name each region (e.g., "product_title", "price")
8. Click "Finish & Save"

### Applying a Template

1. Navigate to a page with content matching your template
2. Open the extension popup
3. Go to the "Apply" tab
4. Select a template from the dropdown
5. Click "Extract Data"
6. View and copy the extracted results

### Managing Templates

- **View**: Click "Templates" tab to see all saved templates
- **Edit**: Click "Edit" on any template to modify name or view regions
- **Export**: Export templates as JSON for backup or sharing
- **Import**: Import templates from JSON files
- **Delete**: Remove templates you no longer need

## Architecture

### Components

- **manifest.json**: Extension configuration (Manifest V3)
- **service-worker.ts**: Background service worker handling template storage and backend communication
- **content-script.ts**: DOM interaction, element selection, recording UI
- **popup.ts**: Popup UI logic and user interactions
- **popup.html**: Extension popup interface
- **popup.css**: Styling for the UI
- **types.ts**: TypeScript type definitions

### Communication Flow

```
Popup UI → Service Worker → Backend MCP Server (via HTTP POST)
         ↓
Content Script (DOM Interaction)
```

### Storage

Templates are stored in Chrome's local storage:

```javascript
chrome.storage.local.set({
  'stygian_plugin_templates': [/* array of templates */]
});
```

## API Communication

The extension communicates with the Stygian MCP backend via HTTP POST requests:

```javascript
POST http://localhost:3000/mcp/tools/call
{
  "jsonrpc": "2.0",
  "id": 123,
  "method": "tools/call",
  "params": {
    "name": "plugin_apply_template",
    "arguments": {
      "template_id": "uuid",
      "html": "...",
      "url": "..."
    }
  }
}
```

## Browser Compatibility

- Chrome/Chromium 88+
- Edge 88+ (same as Chromium)

## Security

- **Content Security Policy**: Restricted to prevent XSS attacks
- **DOM Access**: Limited to pages the user navigates to
- **Storage**: Templates stored locally, not transmitted unless explicitly exported
- **No Network Requests**: Communication only happens when user explicitly applies templates

## Troubleshooting

### Extension not showing?

- Check that Developer Mode is enabled in `chrome://extensions`
- Ensure the extension is enabled (toggle should be blue)
- Try refreshing the page

### Recording not working?

- Make sure you're on a regular webpage (not chrome://, chrome-extension://, etc.)
- Check the content script is loaded (look in DevTools console)
- Try reloading the page

### No data extracted?

- Verify the template's CSS selectors match elements on the page
- Check the extraction results in the popup for error messages
- Try inspecting the page to confirm element structure hasn't changed

### Backend connection issues?

- Ensure the Stygian MCP backend is running and accessible
- Check the network tab in DevTools for failed requests
- Verify the backend URL in service-worker.ts is correct

## Development

### TypeScript Compilation

```bash
# Watch mode for development
npx tsc --watch

# Single compilation
npx tsc

# Output goes to dist/ directory
```

### Debugging

1. Open `chrome://extensions`
2. Click "Details" on the Stygian Plugin extension
3. Click "Inspect views: service worker" for background script
4. Click on any page and right-click → "Inspect" → "Console" for content script
5. View popup logs: Click extension icon → Right-click on popup → "Inspect"

### Hot Reload

After making changes to TypeScript:

1. Run `npx tsc` to compile
2. Go to `chrome://extensions`
3. Click the refresh icon on the Stygian Plugin card

## Future Features

- [ ] Batch extraction from multiple pages
- [ ] Custom JavaScript transformations
- [ ] Pattern learning from multiple examples
- [ ] Cloud sync for team templates
- [ ] API for programmatic access
- [ ] Screenshot annotation UI
- [ ] XPath/CSS selector generation with AI
- [ ] Template versioning and history

## Contributing

To contribute to the extension:

1. Fork the stygian repository
2. Create a feature branch
3. Make your changes to the TypeScript files
4. Test locally with `npm run build` and manual testing
5. Submit a pull request

## License

AGPL-3.0-only OR LicenseRef-Commercial

See the main stygian repository for licensing details.
