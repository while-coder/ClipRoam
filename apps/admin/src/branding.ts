const names = {
  en: { displayName: "ClipRoam" },
  zh: { displayName: "随心贴" },
};

export const displayName = names[/^zh(?:[-_]|$)/i.test(navigator.language) ? "zh" : "en"].displayName;
