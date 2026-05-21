import fs from "node:fs";
import path from "node:path";
import type { IncomingMessage, ServerResponse } from "node:http";
import { defineConfig, type Plugin } from "vite";
import react from "@vitejs/plugin-react";

const repoRoot = path.resolve(__dirname, "..");
const resultsRoot = path.resolve(repoRoot, "results");

function resultsPlugin(): Plugin {
  function handleResultsRequest(requestUrl: string, response: ServerResponse): boolean {
    if (!requestUrl.startsWith("/results/")) {
      return false;
    }

    const relativePath = requestUrl.replace(/^\/results\//, "");
    const filePath = path.resolve(resultsRoot, relativePath);
    if (!filePath.startsWith(resultsRoot)) {
      response.statusCode = 403;
      response.end("Forbidden");
      return true;
    }

    if (!fs.existsSync(filePath) || fs.statSync(filePath).isDirectory()) {
      response.statusCode = 404;
      response.end("Not Found");
      return true;
    }

    response.setHeader("Content-Type", contentType(filePath));
    response.end(fs.readFileSync(filePath));
    return true;
  }

  function middleware(req: IncomingMessage, res: ServerResponse, next: () => void) {
    const requestUrl = req.url?.split("?")[0] ?? "";
    if (!handleResultsRequest(requestUrl, res)) {
      next();
    }
  }

  return {
    name: "serve-results-directory",
    configureServer(server) {
      server.middlewares.use(middleware);
    },
    configurePreviewServer(server) {
      server.middlewares.use(middleware);
    },
  };
}

function contentType(filePath: string): string {
  const extension = path.extname(filePath).toLowerCase();
  switch (extension) {
    case ".json":
      return "application/json; charset=utf-8";
    case ".html":
      return "text/html; charset=utf-8";
    case ".css":
      return "text/css; charset=utf-8";
    case ".js":
    case ".mjs":
      return "application/javascript; charset=utf-8";
    case ".md":
    case ".log":
    case ".ndjson":
    case ".txt":
    case ".svg":
      return "text/plain; charset=utf-8";
    default:
      return "application/octet-stream";
  }
}

export default defineConfig({
  base: "./",
  plugins: [react(), resultsPlugin()],
});
