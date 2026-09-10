import { createSecureContext } from "node:tls";
import { existsSync, mkdirSync, readFileSync, renameSync, rmSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { tlsDirectory } from "../DataPaths.js";
import { TLS_MAX_PEM_BYTES } from "../app/ServerConfig.js";

export type TlsOptions = { cert: Buffer; key: Buffer };
export type TlsStatus = { enabled: boolean; source: "managed" | "none" };

const CERT_FILE = join(tlsDirectory, "cert.pem");
const KEY_FILE = join(tlsDirectory, "key.pem");

export class TlsCertificateService {
  #options: TlsOptions | undefined;
  #source: TlsStatus["source"] = "none";

  constructor() {
    if (existsSync(CERT_FILE) !== existsSync(KEY_FILE)) {
      throw new Error("Managed TLS certificate and key files must both exist.");
    }
    if (existsSync(CERT_FILE)) this.#setOptions(readCertificateFiles(CERT_FILE, KEY_FILE), "managed");
  }

  get options(): TlsOptions | undefined { return this.#options; }
  get status(): TlsStatus { return { enabled: Boolean(this.#options), source: this.#source }; }

  replace(cert: unknown, key: unknown): TlsOptions {
    if (typeof cert !== "string" || typeof key !== "string" || !cert.trim() || !key.trim()) {
      throw new Error("Certificate and private key are required.");
    }
    if (Buffer.byteLength(cert) > TLS_MAX_PEM_BYTES || Buffer.byteLength(key) > TLS_MAX_PEM_BYTES) {
      throw new Error("Certificate or private key is too large.");
    }

    const options = { cert: Buffer.from(cert), key: Buffer.from(key) };
    validateOptions(options);
    mkdirSync(tlsDirectory, { recursive: true });
    writeAtomically(CERT_FILE, options.cert, 0o644);
    writeAtomically(KEY_FILE, options.key, 0o600);
    this.#setOptions(options, "managed");
    return options;
  }

  remove(): void {
    if (this.#source !== "managed") {
      throw new Error("No managed TLS certificate is configured.");
    }

    rmSync(CERT_FILE, { force: true });
    rmSync(KEY_FILE, { force: true });
    this.#options = undefined;
    this.#source = "none";
  }

  #setOptions(options: TlsOptions, source: TlsStatus["source"]): void {
    validateOptions(options);
    this.#options = options;
    this.#source = source;
  }
}

function readCertificateFiles(certPath: string, keyPath: string): TlsOptions {
  return { cert: readFileSync(certPath), key: readFileSync(keyPath) };
}

function validateOptions(options: TlsOptions): void {
  createSecureContext(options);
}

function writeAtomically(path: string, contents: Buffer, mode: number): void {
  const temporaryPath = `${path}.${process.pid}.new`;
  writeFileSync(temporaryPath, contents, { mode });
  renameSync(temporaryPath, path);
}
