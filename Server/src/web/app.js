const game = document.querySelector("#game");
  const playerId = document.querySelector("#playerId");
  const nameInput = document.querySelector("#name");
  const connection = document.querySelector("#connection");
  const leaveHeader = document.querySelector("#leaveHeader");
  const actionWindow = document.querySelector("#actionWindow");
  const actionHandle = document.querySelector("#actionHandle");
  let joined = false;
  let socket = null;
  let refreshInFlight = false;
  let refreshQueued = false;
  let selectedCards = new Set();
  let pendingRaise = 0;
  let hasSeenRunning = false;
  let autoKeepPlayingKey = "";

  function ensureClientName() {
    if (!localStorage.getItem("pokerClientName")) {
    localStorage.setItem("pokerClientName", "Player");
    }
    playerId.value = "";
    playerId.placeholder = "Assigned on join";
    nameInput.value = localStorage.getItem("pokerClientName");
  }

  async function api(path, options = {}) {
    const response = await fetch(path, {
    headers: { "content-type": "application/json" },
    ...options,
    });
    const body = await response.json();
    if (!response.ok || body.ok === false) throw new Error(body.error || "Request failed");
    return body;
  }

  function cardLabel(value) {
    const rank = value & 0x0f;
    const suit = (value >> 4) & 0x03;
    const suits = ["♦", "♣", "♥", "♠"];
    const labels = {1: "A", 11: "J", 12: "Q", 13: "K"};
    return `${labels[rank] || rank}${suits[suit] || ""}`;
  }

  function cardClass(value) {
    if (!value) return "card hidden";
    const suit = (value >> 4) & 0x03;
    return `card ${suit === 0 || suit === 2 ? "red" : ""}`;
  }

  function escapeHtml(value) {
    return String(value).replace(/[&<>"']/g, char => ({
    "&": "&amp;",
    "<": "&lt;",
    ">": "&gt;",
    '"': "&quot;",
    "'": "&#39;",
    }[char]));
  }

  function chipBreakdown(amount) {
    const total = Math.max(0, Number(amount || 0));
    const denominations = [500, 100, 25, 10];
    const best = Array(total + 1).fill(null);
    best[0] = [];

    for (let value = 1; value <= total; value += 1) {
    denominations.forEach(denomination => {
      if (value >= denomination && best[value - denomination]) {
      const candidate = [...best[value - denomination], denomination];
      if (!best[value] || candidate.length < best[value].length) {
        best[value] = candidate;
      }
      }
    });
    }

    const values = best[total] || (total ? [total] : []);
    const counts = values.reduce((acc, value) => {
    acc[value] = (acc[value] || 0) + 1;
    return acc;
    }, {});
    return Object.entries(counts)
    .map(([value, count]) => ({ value: Number(value), count }))
    .sort((a, b) => b.value - a.value);
  }

  function renderMoney(target, label, amount) {
    const total = Number(amount || 0);
    const chips = chipBreakdown(total);
    target.innerHTML = `<div class="money-label">${label}: ${total}</div>
    <div class="money-chips">
      ${chips.length ? chips.map(chip => `<span class="chip-unit"><span class="chip denom-${chip.value}">${chip.value}</span>${chip.count > 1 ? `<span class="chip-count">x${chip.count}</span>` : ""}</span>`).join("") : `<span class="chip-count">0</span>`}
    </div>`;
  }

  function renderPot(amount) {
    document.querySelector("#potTotal").textContent = amount ?? 0;
    const potChips = document.querySelector("#potChips");
    const chips = chipBreakdown(amount);
    potChips.innerHTML = chips.length
    ? chips.map(chip => `<span class="chip-unit"><span class="chip denom-${chip.value}">${chip.value}</span>${chip.count > 1 ? `<span class="chip-count">x${chip.count}</span>` : ""}</span>`).join("")
    : `<span class="chip-count">0</span>`;
  }

  function updateRaisePreview() {
    renderMoney(document.querySelector("#raisePreview"), "Raise", pendingRaise);
    document.querySelector("#raise").disabled = pendingRaise <= 0;
  }

  function addRaise(amount) {
    pendingRaise += amount;
    updateRaisePreview();
  }

  function clearRaise() {
    pendingRaise = 0;
    updateRaisePreview();
  }

  function renderCards(target, cards = [], options = {}) {
    target.innerHTML = "";
    cards.forEach(card => {
    const el = document.createElement("span");
    const selectable = options.selectable && card;
    const selected = selectable && selectedCards.has(card);
    const rankCard = card && options.rankCards?.has(card);
    const rankClass = rankCard ? (options.losingRankCards ? "rank-card-loser" : "rank-card") : "";
    el.className = `${cardClass(card)} ${selectable ? "selectable" : ""} ${selected ? "selected" : ""} ${rankClass}`;
    el.textContent = card ? cardLabel(card) : "Hidden";
    if (selectable) {
      el.addEventListener("click", () => {
      if (selectedCards.has(card)) {
        selectedCards.delete(card);
      } else {
        selectedCards.add(card);
      }
      updateReplaceButton();
      renderCards(target, cards, options);
      });
    }
    target.appendChild(el);
    });
  }

  function showdownId(state) {
    if (!state.showdown) return "";
    const playerIds = (state.showdown.players || []).map(player => player.id).join("-");
    return `${state.showdown.winner}:${playerIds}:${state.game?.round ?? 0}`;
  }

  function autoKeepPlaying(state) {
    const key = showdownId(state);
    if (!joined || !state.me || !key || key === autoKeepPlayingKey) return;

    autoKeepPlayingKey = key;
    send("KEEP_PLAYING")
    .then(() => requestRefresh().catch(() => {}))
    .catch(() => {});
  }

  function updateReplaceButton() {
    document.querySelector("#replaceCards").disabled = selectedCards.size === 0;
  }

  function isReplacePhase(state) {
    return game.value === "FiveCardDraw" && state.game?.round === 1;
  }

  function updateActions(state) {
    const isMyTurn = joined && state.me && state.turn === state.me.id;
    const canAct = Boolean(isMyTurn && state.is_running);
    const replacePhase = canAct && isReplacePhase(state);
    const betPhase = canAct && !replacePhase;

    actionWindow.classList.toggle("is-minimized", !canAct);
    document.querySelector("#waitingHint").classList.add("is-hidden");
    document.querySelector("#betActions").classList.toggle("is-hidden", !betPhase);
    document.querySelector("#replaceActions").classList.toggle("is-hidden", !replacePhase);
    if (!betPhase) clearRaise();
  }

  function showStart() {
    document.querySelector("#startView").classList.remove("is-hidden");
    document.querySelector("#gameView").classList.add("is-hidden");
    leaveHeader.classList.add("is-hidden");
    actionWindow.classList.add("is-hidden");
  }

  function showTable() {
    document.querySelector("#startView").classList.add("is-hidden");
    document.querySelector("#gameView").classList.remove("is-hidden");
    leaveHeader.classList.remove("is-hidden");
    actionWindow.classList.remove("is-hidden");
    document.querySelector("#gameTitle").textContent = game.value;
  }

  function renderBoard(state) {
    const board = document.querySelector("#board");
    const cards = state.game?.community_cards || [];
    if (cards.length) {
    const winner = state.showdown?.players?.find(player => player.id === state.showdown?.winner);
    renderCards(board, cards, { rankCards: new Set(winner?.rank_cards || []) });
    return;
    }

    board.innerHTML = "";
    const empty = document.createElement("span");
    empty.className = "empty-board";
    empty.textContent = "No table cards";
    board.appendChild(empty);
  }

  function render(state) {
    renderPot(state.pot ?? 0);
    const currentHand = new Set(state.me?.hand ?? []);
    selectedCards = new Set([...selectedCards].filter(card => currentHand.has(card)));
    updateReplaceButton();
    updateActions(state);
    renderBoard(state);

    if (joined && state.is_running) {
    hasSeenRunning = true;
    autoKeepPlayingKey = "";
    }

    const players = document.querySelector("#players");
    const tableHands = document.querySelector("#tableHands");
    players.innerHTML = "";
    tableHands.innerHTML = "";
    (state.players || []).forEach(player => {
    const isSelf = state.me?.id === player.id;
    const showdownPlayer = state.showdown?.players?.find(showdownPlayer => showdownPlayer.id === player.id);
    const isShowdownLoser = Boolean(showdownPlayer && state.showdown?.winner !== player.id);
    const showdownCards = showdownPlayer?.hand || null;
    const visibleCards = showdownCards || (isSelf ? state.me?.hand || [] : player.hand || []);
    const rankCards = new Set(showdownPlayer?.rank_cards || []);
    const communityCards = state.game?.community_cards || [];
    const usedCommunityCards = (showdownPlayer?.rank_cards || []).filter(card => communityCards.includes(card));
    const el = document.createElement("article");
    el.className = `player ${player.id === state.turn ? "current" : ""} ${isSelf ? "self" : ""} ${state.showdown?.winner === player.id ? "winner" : ""}`;
    el.innerHTML = `<h3>${escapeHtml(player.name)} ${isSelf ? '<span class="me-tag">(ME)</span>' : ''}</h3>
      <div class="chip-row wallet-chips"></div>
      <div>${player.folded ? "Folded" : "Active"}</div>`;
    renderMoney(el.querySelector(".wallet-chips"), "Wallet", player.wallet);
    players.appendChild(el);

    const hand = document.createElement("div");
    hand.className = `table-hand ${player.id === state.turn ? "current" : ""} ${isSelf ? "self" : ""}`;
    hand.innerHTML = `<div>
      <div class="cards"></div>
      <div class="rank-label">${showdownPlayer?.rank || ""}</div>
      <div class="cards rank-used"></div>
      </div>
      <div class="table-bet"></div>`;
    renderCards(hand.querySelector(".cards"), visibleCards, {
      selectable: !state.showdown && isSelf && isReplacePhase(state) && state.turn === state.me?.id,
      rankCards,
      losingRankCards: isShowdownLoser,
    });
    renderMoney(hand.querySelector(".table-bet"), "Bet", player.bet);
    if (usedCommunityCards.length) {
      renderCards(hand.querySelector(".rank-used"), usedCommunityCards, {
        rankCards,
        losingRankCards: isShowdownLoser,
      });
    }
    tableHands.appendChild(hand);
    });

    if (joined && state.me && hasSeenRunning && state.showdown) {
    autoKeepPlaying(state);
    }
  }

  async function refresh() {
    if (!joined || !game.value) return;
    const id = joined ? playerId.value : "";
    const body = await api(`/api/games/${game.value}/state${id ? `?player_id=${id}` : ""}`);
    window.latestState = body.state;
    render(body.state);
    connection.textContent = joined ? `Joined ${game.value}` : "Connected";
  }

  async function requestRefresh() {
    if (refreshInFlight) {
    refreshQueued = true;
    return;
    }

    refreshInFlight = true;
    try {
    await refresh();
    } finally {
    refreshInFlight = false;
    }

    if (refreshQueued) {
    refreshQueued = false;
    requestRefresh();
    }
  }

  function connectSocket() {
    if (socket && (socket.readyState === WebSocket.OPEN || socket.readyState === WebSocket.CONNECTING)) return;

    const protocol = location.protocol === "https:" ? "wss" : "ws";
    socket = new WebSocket(`${protocol}://${location.host}/ws`);
    socket.addEventListener("open", () => {
    connection.textContent = "Connected";
    requestRefresh().catch(() => {});
    });
    socket.addEventListener("message", () => requestRefresh().catch(() => {}));
    socket.addEventListener("close", () => {
    connection.textContent = "Disconnected";
    setTimeout(connectSocket, 1000);
    });
    socket.addEventListener("error", () => socket.close());
  }

  async function loadGames() {
    const body = await api("/api/games");
    game.innerHTML = "";
    body.games.forEach(name => {
    const option = document.createElement("option");
    option.value = name;
    option.textContent = name;
    game.appendChild(option);
    });
  }

  async function join() {
    localStorage.setItem("pokerClientName", nameInput.value);
    const body = await api(`/api/games/${game.value}/join`, {
    method: "POST",
    body: JSON.stringify({ name: nameInput.value, wallet: 1000 }),
    });
    joined = true;
    playerId.value = body.player_id;
    hasSeenRunning = false;
    autoKeepPlayingKey = "";
    showTable();
    window.latestState = body.state;
    render(body.state);
  }

  async function send(command) {
    const body = await api(`/api/games/${game.value}/players/${playerId.value}/action`, {
    method: "POST",
    body: JSON.stringify({ command }),
    });
    window.latestState = body.state;
    render(body.state);
  }

  async function replaceSelected() {
    if (!selectedCards.size) return;
    const command = `REPLACE_CARDS ${[...selectedCards].join(" ")}`;
    selectedCards.clear();
    updateReplaceButton();
    await send(command);
  }

  async function raisePending() {
    if (pendingRaise <= 0) return;
    const amount = pendingRaise;
    clearRaise();
    await send(`RAISE ${amount}`);
  }

  async function standPat() {
    selectedCards.clear();
    updateReplaceButton();
    await send("REPLACE_CARDS");
  }

  async function leaveTable() {
    if (joined) {
    try {
      await send("LEAVE");
    } catch (_) {}
    }

    joined = false;
    hasSeenRunning = false;
    autoKeepPlayingKey = "";
    selectedCards.clear();
    clearRaise();
    updateReplaceButton();
    updateActions({ is_running: false, turn: null, me: null, game: {} });
    connection.textContent = "Connected";
    showStart();
  }

  document.querySelector("#join").addEventListener("click", () => join().catch(alert));
  document.querySelector("#check").addEventListener("click", () => send("CHECK").catch(alert));
  document.querySelector("#fold").addEventListener("click", () => send("FOLD").catch(alert));
  document.querySelectorAll("[data-chip]").forEach(button => {
    button.addEventListener("click", () => addRaise(Number(button.dataset.chip)));
  });
  document.querySelector("#clearRaise").addEventListener("click", clearRaise);
  document.querySelector("#raise").addEventListener("click", () => raisePending().catch(alert));
  document.querySelector("#replaceCards").addEventListener("click", () => replaceSelected().catch(alert));
  document.querySelector("#standPat").addEventListener("click", () => standPat().catch(alert));
  leaveHeader.addEventListener("click", () => leaveTable().catch(alert));
  game.addEventListener("change", () => {
    document.querySelector("#gameTitle").textContent = game.value || "Table";
  });
  function clampActionWindow(left, top) {
    const margin = 8;
    const maxLeft = Math.max(margin, window.innerWidth - actionWindow.offsetWidth - margin);
    const maxTop = Math.max(margin, window.innerHeight - actionWindow.offsetHeight - margin);
    actionWindow.style.left = `${Math.max(margin, Math.min(left, maxLeft))}px`;
    actionWindow.style.top = `${Math.max(margin, Math.min(top, maxTop))}px`;
  }

  actionHandle.addEventListener("pointerdown", event => {
    const rect = actionWindow.getBoundingClientRect();
    const offsetX = event.clientX - rect.left;
    const offsetY = event.clientY - rect.top;
    actionHandle.setPointerCapture(event.pointerId);

    const onMove = moveEvent => {
      clampActionWindow(moveEvent.clientX - offsetX, moveEvent.clientY - offsetY);
    };

    const onUp = upEvent => {
      actionHandle.releasePointerCapture(upEvent.pointerId);
      actionHandle.removeEventListener("pointermove", onMove);
      actionHandle.removeEventListener("pointerup", onUp);
      actionHandle.removeEventListener("pointercancel", onUp);
    };

    actionHandle.addEventListener("pointermove", onMove);
    actionHandle.addEventListener("pointerup", onUp);
    actionHandle.addEventListener("pointercancel", onUp);
  });

  window.addEventListener("resize", () => {
    const rect = actionWindow.getBoundingClientRect();
    clampActionWindow(rect.left, rect.top);
  });
  window.addEventListener("beforeunload", () => {
    if (!joined || !game.value || !playerId.value) return;
    fetch(`/api/games/${game.value}/players/${playerId.value}/action`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ command: "LEAVE" }),
    keepalive: true,
    });
  });
  setInterval(() => requestRefresh().catch(() => connection.textContent = "Disconnected"), 10000);
  ensureClientName();
  updateRaisePreview();
  connectSocket();
  loadGames().catch(alert);
