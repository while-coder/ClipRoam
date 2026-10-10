import { createSecureContext } from "node:tls";
import { existsSync, mkdirSync, readFileSync, rmSync } from "node:fs";
import { join } from "node:path";
import { tlsDirectory } from "../DataPaths.js";
import { TLS_MAX_PEM_BYTES } from "../app/ServerConfig.js";
import { writeFileAtomic } from "../common/atomicWrite.js";

export type TlsOptions = { cert: Buffer; key: Buffer };
export type TlsStatus = { enabled: boolean };

const CERT_FILE = join(tlsDirectory, "cert.pem");
const KEY_FILE = join(tlsDirectory, "key.pem");
const PAIR_FILE = join(tlsDirectory, "pair.json");

export class TlsCertificateService {
  #options: TlsOptions | undefined;

  constructor() {
    // A configured TLS installation must never silently restart in plaintext.
    if (existsSync(PAIR_FILE)) {
      const pair = JSON.parse(readFileSync(PAIR_FILE, "utf8")) as { cert: string; key: string };
      this.#setOptions({ cert: Buffer.from(pair.cert, "base64"), key: Buffer.from(pair.key, "base64") });
    } else if (existsSync(CERT_FILE) && existsSync(KEY_FILE)) {
      this.#setOptions(readCertificateFiles(CERT_FILE, KEY_FILE));
    } else if (existsSync(CERT_FILE) || existsSync(KEY_FILE)) {
      throw new Error("TLS 证书或私钥缺失，请修复 TLS 配置后启动服务。");
    }
  }

  get options(): TlsOptions | undefined { return this.#options; }
  get status(): TlsStatus { return { enabled: Boolean(this.#options) }; }

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
    writeFileAtomic(PAIR_FILE, JSON.stringify({ cert: options.cert.toString("base64"), key: options.key.toString("base64") }), 0o600);
    this.#options = options;
    return options;
  }

  remove(): void {
    if (!this.#options) {
      throw new Error("No managed TLS certificate is configured.");
    }

    rmSync(CERT_FILE, { force: true });
    rmSync(KEY_FILE, { force: true });
    rmSync(PAIR_FILE, { force: true });
    this.#options = undefined;
  }

  #setOptions(options: TlsOptions): void {
    validateOptions(options);
    this.#options = options;
  }

}

function readCertificateFiles(certPath: string, keyPath: string): TlsOptions {
  return { cert: readFileSync(certPath), key: readFileSync(keyPath) };
}

function validateOptions(options: TlsOptions): void {
  createSecureContext(options);
}
