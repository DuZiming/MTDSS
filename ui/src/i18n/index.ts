/// i18n 装配:react-i18next 初始化,V1 仅中文。
import i18n from "i18next";
import { initReactI18next } from "react-i18next";

import zh from "./zh.json";

void i18n.use(initReactI18next).init({
  lng: "zh",
  fallbackLng: "zh",
  resources: {
    zh: { translation: zh },
  },
  interpolation: { escapeValue: false }, // React 已转义
});

export default i18n;
