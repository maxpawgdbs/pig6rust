// =======================================================
// Pig-6 Market API Layer
// Replace only this file when backend becomes available.
// =======================================================

const API_BASE_URL = "/api";

// false = use mock data
// true  = use real backend
const USE_API = true;

// =======================================================

const delay = (ms) => new Promise(resolve => setTimeout(resolve, ms));

// =======================================================
// MOCK DATA
// =======================================================

const mockMarket = {
    price: 42,
    availableCodes: 87,
    capacity: 100,
    priceChange: 4.2,
    status: "ACTIVE"
};

const history = {

    "1H": [
        41, 41, 42, 42, 42, 43, 42, 42, 42, 42, 42, 42
    ],

    "6H": [
        39, 39, 40, 40, 40, 41, 41, 41, 42, 42, 42, 42
    ],

    "24H": [
        34, 35, 35, 36, 36, 37, 38, 39,
        39, 40, 40, 41, 41, 42, 42, 42,
        42, 42, 43, 42, 42, 42, 42, 42
    ],

    "7D": [
        20, 21, 22, 23, 25, 27, 29
    ],

    "30D": [
        10, 11, 12, 14, 15,
        17, 18, 19, 20, 22,
        24, 25, 26, 28, 29,
        30, 31, 32, 33, 34,
        35, 36, 37, 38, 39,
        40, 41, 42, 42, 42
    ]

};

// =======================================================

async function request(url) {

    const response = await fetch(API_BASE_URL + url);

    if (!response.ok) {

        throw new Error("Server error");

    }

    return response.json();

}

// =======================================================

const marketApi = {
    async getTopUsers(limit = 10) {
        const res = await fetch(`/api/top?limit=${limit}`);
        if (!res.ok) throw new Error("Failed to load leaderboard");
        return res.json();
    },

    async getMarket() {

        if (USE_API) {

            return await request("/market");

        }

        await delay(250);

        return structuredClone(mockMarket);

    },

    async getPriceHistory(range = "24H") {

        if (USE_API) {

            return await request("/history?range=" + range);

        }

        await delay(200);

        const values = history[range] || history["24H"];

        return values.map((price, index) => ({

            timestamp: index,

            price

        }));

    },

    async getRecentOperations() {

        if (USE_API) {

            return await request("/operations");

        }

        await delay(180);

        return structuredClone(operations);

    }

};

window.marketApi = marketApi;