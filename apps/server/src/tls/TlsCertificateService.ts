import { createSecureContext } from "node:tls";
import { existsSync, mkdirSync, readFileSync, renameSync, rmSync } from "node:fs";
import { join } from "node:path";
import { tlsDirectory } from "../DataPaths.js";
import { TLS_MAX_PEM_BYTES } from "../app/ServerConfig.js";
import { getLogger } from "../app/Logger.js";
import { writeFileAtomic } from "../common/atomicWrite.js";

const logger = getLogger("TlsCertificateService");

export type TlsOptions = { cert: Buffer; key: Buffer };
export type TlsStatus = { enabled: boolean; source: "managed" | "none" };

const CERT_FILE = join(tlsDirectory, "cert.pem");
const KEY_FILE = join(tlsDirectory, "key.pem");

export class TlsCertificateService {
  #options: TlsOptions | undefined;
  #source: TlsStatus["source"] = "none";

  constructor() {
    // `replace` swaps two files, so a crash or power loss between the two
    // renames can leave a mismatched (or half-present) pair on disk. Throwing
    // here would turn that into a permanent outage — the server would fail
    // every startup until someone deletes the files by hand — so instead the
    // broken pair is quarantined and the server comes up on HTTP, where the
    // admin UI can install a fresh certificate.
    if (existsSync(CERT_FILE) && existsSync(KEY_FILE)) {
      try {
        this.#setOptions(readCertificateFiles(CERT_FILE, KEY_FILE), "managed");
      } catch (error) {
        logger.warn("Managed TLS certificate and key do not match; starting without HTTPS.", error);
        this.#quarantineBrokenPair();
      }
    } else if (existsSync(CERT_FILE) || existsSync(KEY_FILE)) {
      logger.warn("Managed TLS certificate or key is missing; starting without HTTPS.");
      this.#quarantineBrokenPair();
    }
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
    writeFileAtomic(CERT_FILE, options.cert, 0o644);
    writeFileAtomic(KEY_FILE, options.key, 0o600);
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

  // Moves the unusable files aside (kept for inspection, out of the way of a
  // later `replace`) so the service starts clean.
  #quarantineBrokenPair(): void {
    const suffix = `.broken-${Date.now()}`;
    for (const file of [CERT_FILE, KEY_FILE]) {
      if (!existsSync(file)) continue;
      try {
        renameSync(file, `${file}${suffix}`);
      } catch (error) {
        logger.warn(`Could not quarantine ${file}:`, error);
        rmSync(file, { force: true });
      }
    }
  }
}

function readCertificateFiles(certPath: string, keyPath: string): TlsOptions {
  return { cert: readFileSync(certPath), key: readFileSync(keyPath) };
}

function validateOptions(options: TlsOptions): void {
  createSecureContext(options);
}
