/// <reference lib="webworker" />
import { saveShare, shareFromForm } from "./shareInbox";

const worker = self as unknown as ServiceWorkerGlobalScope;
const scope = worker.registration.scope;
const shareTarget = new URL("share-target", scope);

worker.addEventListener("install", (event) => event.waitUntil(worker.skipWaiting()));
worker.addEventListener("activate", (event) => event.waitUntil(worker.clients.claim()));
worker.addEventListener("fetch", (event) => {
  const url = new URL(event.request.url);
  if (event.request.method !== "POST" || url.origin !== shareTarget.origin || url.pathname !== shareTarget.pathname) return;
  event.respondWith((async () => {
    try {
      const share = shareFromForm(await event.request.formData());
      await saveShare(scope, share);
      // Redirect only after IndexedDB commits, so a cold start can immediately import it.
      return Response.redirect(new URL("./", scope).href, 303);
    } catch {
      return new Response("无法保存系统分享，请检查设备存储空间后重新分享。", {
        status: 500, headers: {"Content-Type":"text/plain; charset=utf-8"},
      });
    }
  })());
});
