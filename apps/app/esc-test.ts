// 临时验证页（测完删除）：复现缩略图预览的完整键盘链路——
// 1) App 层 document keydown 守卫（先注册，openModalCount>0 时直接 return）
// 2) HistoryView 式常驻 SModal + :show 驱动，焦点圈围与 Esc 关闭。
import { createApp, h, ref } from "vue";
import SModal from "@qingfeng346/ui-kit/components/SModal.vue";
import { openModalCount } from "@qingfeng346/ui-kit/components/SModal.vue";
import "@qingfeng346/ui-kit/style.css";

// —— App.vue handleKeys 的等价物（先于 SModal 注册）——
document.addEventListener("keydown", (event) => {
  if (openModalCount.value > 0) return; // 模态开着：键盘归模态栈
  if (event.key === "Escape") document.title = "APP-HIDE-WINDOW"; // 不应发生
});

const show = ref(true);

function App() {
  function onUpdateShow(value: boolean) {
    if (!value) {
      show.value = false;
      document.title = "CLOSED";
    }
  }
  return h(
    SModal,
    { show: show.value, title: "缩略图预览测试", width: "980px", "onUpdate:show": onUpdateShow },
    { default: () => h("div", { class: "image-preview-stage" }, "预览内容") },
  );
}

createApp(App).mount("#app");
