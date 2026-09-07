import { ClipRoamServer } from "./app/ClipRoamServer.js";
import { getLogger, logsDirectory, shutdownLogger } from "./app/Logger.js";
import { dataDirectory, usersDirectory } from "./DataPaths.js";

const server = new ClipRoamServer();
const logger = getLogger("server");

let stopping = false;
async function stopServer(signal: string): Promise<void> {
  if (stopping) return;
  stopping = true;
  logger.info(`Shutdown requested via ${signal}; closing server...`);
  try {
    await server.stop();
    logger.info("Server stopped.");
  } finally {
    await shutdownLogger();
  }
}

async function failServer(source: string, error: unknown): Promise<void> {
  logger.fatal(`${source}:`, error);
  try {
    await stopServer(source);
  } catch (stopError) {
    logger.error("Failed to stop server:", stopError);
    await shutdownLogger();
  }
  process.exitCode = 1;
}

try {
  await server.start();
  logger.info(`Server listening on ${server.port}; Admin: ${server.adminUrl}`);
  logger.info(`Data directory: ${dataDirectory}`);
  logger.info(`Users directory: ${usersDirectory}`);
  logger.info(`Logs directory: ${logsDirectory}`);
} catch (error) {
  await failServer("Startup failed", error);
}

for (const signal of ["SIGINT", "SIGTERM"] as const) {
  process.once(signal, () => {
    void stopServer(signal).then(() => process.exit(0)).catch((error) => {
      void failServer(`Shutdown via ${signal} failed`, error);
    });
  });
}

process.once("uncaughtException", (error) => void failServer("Uncaught exception", error));
process.once("unhandledRejection", (reason) => void failServer("Unhandled rejection", reason));
