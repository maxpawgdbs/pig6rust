let chart = null;

const loading = document.getElementById("loading");
const loadingText = document.getElementById("loadingText");
const app = document.getElementById("app");

const errorBox = document.getElementById("errorBox");
const errorText = document.getElementById("errorText");
const retryButton = document.getElementById("retryButton");

const chartFallback = document.getElementById("chartFallback");


const AUTO_REFRESH_MS = 5000;

let currentRange = "24H";
let refreshTimer = null;


// ==============================
// Helpers
// ==============================

function chartAvailable() {
    return typeof Chart !== "undefined";
}


function showApp() {

    loading.classList.add("hidden");
    errorBox.classList.add("hidden");
    app.classList.remove("hidden");

}


function showError(text = "Market unavailable") {

    loading.classList.add("hidden");
    app.classList.add("hidden");

    errorText.textContent = text;

    errorBox.classList.remove("hidden");

}



function formatTime(timestamp) {

    const date = new Date(timestamp);

    if (isNaN(date))
        return timestamp;

    return date.toLocaleTimeString([], {
        hour: "2-digit",
        minute: "2-digit"
    });

}


// ==============================
// Market rendering
// ==============================

// Sets an element's text and briefly flashes it green when the value
// actually changed from the last render — a quiet "this just updated"
// cue for the auto-refreshed numbers, skipped on first paint.
function setText(id, value) {

    const el = document.getElementById(id);

    if (!el)
        return;

    const changed =
        el.dataset.rendered !== undefined &&
        el.dataset.rendered !== String(value);

    el.textContent = value;
    el.dataset.rendered = String(value);

    if (changed) {

        el.classList.remove("flash");

        // force reflow so the animation restarts on rapid updates
        void el.offsetWidth;

        el.classList.add("flash");

        setTimeout(() => el.classList.remove("flash"), 600);

    }

}


function renderMarket(market) {


    setText("currentPrice", market.price);

    setText("currentPrice2", market.price2);

    setText(
        "availableCodes",
        `${market.availableCodes} / ${market.unusedCodes}`
    );


    document.getElementById("marketStatus")
        .textContent = market.status;



    setText("statPrice", `${market.price} P6T`);


    setText("statCodes", market.availableCodes);


    document.getElementById("statCapacity")
        .textContent =
        market.capacity;



    const change =
        document.getElementById("statChange");


    setText(
        "statChange",
        `${market.priceChange > 0 ? "+" : ""}${market.priceChange}%`
    );



    change.style.color =
        market.priceChange >= 0
            ? "#22c55e"
            : "#ef4444";


    document.getElementById("statStatus")
        .textContent =
        market.status;

}



// ==============================
// Chart
// ==============================


async function loadHistory(range) {


    if (!chartAvailable())
        throw new Error("Chart.js missing");



    const history =
        await marketApi.getPriceHistory(range);



    const labels =
        history.map(
            x => formatTime(x.timestamp)
        );


    const prices =
        history.map(
            x => x.price
        );



    const ctx =
        document
            .getElementById("marketChart")
            .getContext("2d");



    const up =
        prices[prices.length - 1] >= prices[0];



    const color =
        up
            ? "#22c55e"
            : "#ef4444";



    if (chart) {


        chart.data.labels = labels;


        chart.data.datasets[0].data =
            prices;


        chart.data.datasets[0].borderColor =
            color;


        chart.data.datasets[0].backgroundColor =
            up
                ? "rgba(34,197,94,0.12)"
                : "rgba(239,68,68,0.12)";


        chart.update("none");

        return;

    }



    chart = new Chart(ctx, {


        type: "line",


        data: {


            labels,


            datasets: [{


                data: prices,


                borderColor: color,


                backgroundColor:
                    up
                        ? "rgba(34,197,94,0.12)"
                        : "rgba(239,68,68,0.12)",



                borderWidth: 2,


                tension: 0.15,


                pointRadius: 0,


                pointHoverRadius: 5,


                fill: true



            }]


        },



        options: {


            responsive: true,


            maintainAspectRatio: false,


            animation: {
                duration: 500
            },


            interaction: {
                intersect: false,
                mode: "index"
            },


            plugins: {


                legend: {
                    display: false
                }


            },



            scales: {


                x: {


                    ticks: {
                        color: "#777",
                        maxTicksLimit: 6
                    },


                    grid: {
                        display: false
                    }


                },



                y: {


                    ticks: {
                        color: "#777"
                    },


                    grid: {
                        color: "#1f1f1f"
                    }


                }


            }


        }



    });



    chartFallback.classList.add("hidden");

}



// ==============================
// Leaderboard
// ==============================
const MEDALS = ["🥇", "🥈", "🥉"];

let currentLeaderboardLimit = 10;

async function loadLeaderboard(limit = currentLeaderboardLimit) {

    const listEl = document.getElementById("leaderboardList");

    if (!listEl) return;

    try {

        const users = await marketApi.getTopUsers(limit);

        if (!users.length) {
            listEl.innerHTML =
                `<div class="leaderboard-row">Пока здесь никого нет.</div>`;
            return;
        }

        listEl.innerHTML = users.map(u => `
            <div class="leaderboard-row">
                <div class="leaderboard-rank">
                    <span class="leaderboard-medal">
                        ${u.rank <= 3 ? MEDALS[u.rank - 1] : u.rank}
                    </span>
                    <span>${u.name}</span>
                </div>
                <span class="leaderboard-balance">${u.balance} P6T</span>
            </div>
        `).join("");

    } catch (e) {

        console.error("Leaderboard failed", e);

        listEl.innerHTML =
            `<div class="leaderboard-row">Leaderboard unavailable.</div>`;

    }

}


function initLeaderboardButtons() {

    document
        .querySelectorAll("#leaderboardButtons [data-limit]")
        .forEach(button => {

            button.onclick = async () => {

                document
                    .querySelectorAll("#leaderboardButtons [data-limit]")
                    .forEach(b => b.classList.remove("active"));

                button.classList.add("active");

                const raw = button.dataset.limit;

                currentLeaderboardLimit =
                    raw === "all" ? 1000 : Number(raw);

                await loadLeaderboard(currentLeaderboardLimit);

            };

        });

}


// ==============================
// Refresh
// ==============================

async function refresh() {


    try {


        const market =
            await marketApi.getMarket();


        renderMarket(market);


    }
    catch (e) {

        console.error(
            "Market refresh failed",
            e
        );

    }



    try {


        await loadHistory(currentRange);


    }
    catch (e) {

        console.error(
            "Chart refresh failed",
            e
        );

    }


    try {
        await loadLeaderboard();
    }
    catch (e) {
        console.error("Leaderboard refresh failed", e);
    }

}




function startRefresh() {

    if (refreshTimer)
        clearInterval(refreshTimer);


    refreshTimer =
        setInterval(
            refresh,
            AUTO_REFRESH_MS
        );

}



// ==============================
// Buttons
// ==============================

function initButtons() {


    document
        .querySelectorAll("[data-range]")
        .forEach(button => {


            button.onclick = async () => {


                document
                    .querySelectorAll("[data-range]")
                    .forEach(b =>
                        b.classList.remove("active")
                    );


                button.classList.add("active");


                currentRange =
                    button.dataset.range;



                await loadHistory(
                    currentRange
                );


            };


        });



    retryButton.onclick = () => {

        location.reload();

    };


}



// ==============================
// Start
// ==============================

async function init() {


    try {


        const market =
            await marketApi.getMarket();



        renderMarket(market);



        await loadHistory(
            currentRange
        );


        loadLeaderboard();
        initLeaderboardButtons();

        initButtons();


        showApp();


        startRefresh();



    }
    catch (e) {


        console.error(e);


        showError(
            "Market data unavailable"
        );


    }

}


init(); 