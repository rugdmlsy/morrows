(() => {
  const form = document.getElementById("oauth-consent");
  const approveButton = document.getElementById("oauth-approve");
  const status = document.getElementById("oauth-status");
  const command = document.getElementById("oauth-command");
  if (!form || !approveButton || !status || !command) return;

  const needsAdmin = form.dataset.requiresAdmin === "true";
  const body = () => new URLSearchParams(new FormData(form));

  async function approve(token) {
    const headers = {
      "Content-Type": "application/x-www-form-urlencoded",
      "Accept": "application/json",
    };
    if (token) headers.Authorization = "Bearer " + token;
    const response = await fetch(form.action, {
      method: "POST",
      headers,
      body: body(),
      credentials: "same-origin",
    });
    if (response.ok) {
      const result = await response.json();
      window.location.assign(result.redirect_to);
      return true;
    }
    if (response.status === 401 || response.status === 403) return false;
    let message = "OAuth approval failed (" + response.status + ")";
    try {
      const result = await response.json();
      if (result.error) message = result.error;
    } catch {}
    throw new Error(message);
  }

  function browserTokens() {
    const values = [];
    for (const storage of [window.sessionStorage, window.localStorage]) {
      for (const key of ["morrows.oauth_access_token", "morrows.operatorToken"]) {
        const value = storage.getItem(key);
        if (value && !values.includes(value)) values.push(value);
      }
    }
    return values;
  }

  async function startOperatorLogin() {
    const response = await fetch("/morrows/api/operator-login", {
      method: "POST",
      headers: {"Content-Type": "application/json", "Accept": "application/json"},
      body: JSON.stringify({label: "OAuth consent"}),
      credentials: "same-origin",
    });
    if (!response.ok) throw new Error("Operator login request failed (" + response.status + ")");
    const login = await response.json();
    const roleArg = needsAdmin ? " --role admin" : "";
    status.textContent = needsAdmin
      ? "Admin approval required. Login code: " + login.code
      : "Operator approval required. Login code: " + login.code;
    command.textContent =
      "ssh ovh-vps 'cd /srv/morrow/workspaces/morrows && ./target/release/morrows operator-approve --database data/morrows.db --code "
      + login.code + roleArg + "'";

    async function poll() {
      const pollResponse = await fetch("/morrows/api/operator-login/status", {
        method: "POST",
        headers: {"Content-Type": "application/json", "Accept": "application/json"},
        body: JSON.stringify({id: login.id, token: login.token}),
        credentials: "same-origin",
      });
      if (!pollResponse.ok) throw new Error("Operator login status failed (" + pollResponse.status + ")");
      const result = await pollResponse.json();
      if (result.status === "approved") {
        window.sessionStorage.setItem("morrows.operatorToken", login.token);
        status.textContent = "Operator login approved. Completing OAuth authorization...";
        if (!(await approve(login.token))) {
          throw new Error(needsAdmin
            ? "The approved operator credential is not an admin credential."
            : "The approved operator credential cannot authorize this client.");
        }
        return;
      }
      if (result.status === "expired") throw new Error("Operator login request expired.");
      window.setTimeout(() => poll().catch(showError), 1500);
    }
    window.setTimeout(() => poll().catch(showError), 500);
  }

  function showError(error) {
    approveButton.disabled = false;
    status.textContent = error instanceof Error ? error.message : String(error);
  }

  approveButton.addEventListener("click", async () => {
    approveButton.disabled = true;
    status.textContent = "Checking Morrows operator login...";
    command.textContent = "";
    try {
      if (await approve("")) return;
      for (const token of browserTokens()) {
        if (await approve(token)) return;
      }
      await startOperatorLogin();
    } catch (error) {
      showError(error);
    }
  });
})();
