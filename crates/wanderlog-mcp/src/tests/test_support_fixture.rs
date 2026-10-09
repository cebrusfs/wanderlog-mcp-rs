use serde_json::{Value, json};

/// A small synthetic trip shaped like captured Wanderlog documents (no real data).
pub fn doc() -> Value {
    json!({
        "title": "Test trip",
        "startDate": "2026-11-10", "endDate": "2026-11-11", "days": 2,
        "itinerary": {
            "options": {},
            "budget": {"amount": {"amount": 0, "currencyCode": "USD"}, "expenses": [], "payments": [], "simplifyDebt": false},
            "journal": {"stops": [], "summary": ""},
            "sections": [
                {"heading": "Notes", "text": {"ops": [{"insert": "Bring cash\n"}]}, "blocks": [],
                 "placeMarkerColor": "#000000", "placeMarkerIcon": "map-marker", "id": 100, "type": "textOnly", "mode": "placeList"},
                {"heading": "Places to visit", "text": {"ops": [{"insert": "\n"}]}, "placeMarkerColor": "#3f52e3",
                 "placeMarkerIcon": "map-marker", "id": 101, "type": "normal", "mode": "placeList", "date": null,
                 "blocks": [{"id": 1, "type": "place", "place": {"name": "Sensō-ji", "place_id": "P1", "rating": 4.5, "types": ["tourist_attraction"]},
                             "text": {"ops": [{"insert": "\n"}]}, "addedBy": {"type": "user", "userId": 7}, "imageSize": "small",
                             "upvotedBy": [], "travelMode": null, "attachments": []}]},
                {"heading": "", "text": {"ops": [{"insert": "\n"}]}, "blocks": [], "placeMarkerColor": "#46cdcf",
                 "placeMarkerIcon": "map-marker", "id": 102, "type": "normal", "mode": "dayPlan", "date": "2026-11-10"},
                {"heading": "Museums", "text": {"ops": [{"insert": "\n"}]}, "placeMarkerColor": "#7045af",
                 "placeMarkerIcon": "map-marker", "id": 103, "type": "normal", "mode": "dayPlan", "date": "2026-11-11",
                 "blocks": [
                    {"id": 2, "type": "place", "place": {"name": "Tokyo Tower", "place_id": "P2"}, "text": {"ops": [{"insert": "Go at sunset.\n"}]},
                     "startTime": "17:00", "endTime": null, "addedBy": {"type": "user", "userId": 7}, "imageSize": "small",
                     "upvotedBy": [], "travelMode": null, "attachments": []},
                    {"id": 3, "type": "note", "text": {"ops": [{"insert": "Buy tickets\n"}]}, "addedBy": {"type": "user", "userId": 7}, "attachments": []}
                 ]}
            ]
        }
    })
}
