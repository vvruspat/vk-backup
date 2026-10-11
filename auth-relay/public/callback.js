import { localCallback } from "./relay.js";

const incoming = location.href;
history.replaceState(null, "", "/callback");
const heading = document.getElementById("heading");
const message = document.getElementById("message");
const button = document.getElementById("return");

try {
  const target = localCallback(incoming);
  heading.textContent = "Возвращаемся в VK Backup";
  message.textContent =
    "Программа должна быть открыта на этом компьютере. Если переход не сработал, нажмите кнопку ниже.";
  button.href = target;
  button.hidden = false;
  // A top-level navigation avoids mixed-content fetches and local-network CORS requirements.
  location.replace(target);
} catch (error) {
  heading.textContent = "Откройте VK Backup";
  message.textContent = error.message;
}
