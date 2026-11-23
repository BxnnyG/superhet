# Weather satellite API

`/api/v1/weather/...`, session cookie, mutating calls need a same-origin `Origin` and a JSON body.
Design: ADR-0034, behaviour: `docs/plugins-weather.md`. Errors are `{"v":1,"ok":false,"code","error"}`.

| Route | Role | What |
|---|---|---|
| `GET /weather/config` | account (no guests) | satellite catalogue with status, element sets, store usage |
| `POST /weather/passes` | account | upcoming passes |
| `GET /weather/captures?dev=&limit=&before_ms=` | account | gallery, newest first |
| `GET /weather/captures/{id}` | account | one capture |
| `GET /weather/captures/{id}/images/{name}?thumb=&download=` | account | PNG |
| `PATCH /weather/captures/{id}` `{"keep":bool}` | operator | protect from the quota cleanup |
| `DELETE /weather/captures/{id}` | operator | delete the pass |
| `PUT /admin/weather/elements` `{"enabled":bool}` | admin | allow / forbid the download (allowing fetches at once) |
| `POST /admin/weather/elements/refresh` | admin | download now (429 `too_soon` inside 2 hours, 409 `download_off`) |
| `POST /admin/weather/elements/manual` `{"text":"..."}` | admin | use pasted TLE / OMM JSON |

Captures of a device need the device policy's *view* (list, show, images) or *control* (keep, delete)
for that device (ADR-0023), like recordings. Captures without a device (imports) need only the role.

## Passes

Request `{"dev"?:"dev0","lat"?:51.0,"lon"?:7.0,"hours"?:24,"min_elevation_deg"?:10}`: `hours` 1 to 72,
elevation 0 to 85. Without `lat` / `lon` the position ADS-B stored for `dev` is used. Reply:

`{"v":1,"generated_ms","position_from":"request"|"adsb","elements":{"source","fetched_ms"},
"passes":[{"satellite","norad_id","format","aos_ms","tca_ms","los_ms","max_elevation_deg",
"aos_azimuth_deg","tca_azimuth_deg","los_azimuth_deg","in_progress","downlinks":[{"hz","label"}],
"elements_age_h"}]}`, soonest first. Only satellites that are in service are predicted. The
position is never part of a reply. Codes: `no_elements` (409; nobody allowed the download or pasted
data), `no_position` (409).

## Captures

`{"id":"<unix s>-<4 hex>","satellite","norad_id","pipeline":"lrpt"|"apt","started_ms","duration_s",
"frequency_hz","device","max_elevation_deg" (null until computed),"peak_db","keep","size_bytes",
"images":[{"name","label","width","height","size","thumb"}]}`. `?thumb=true` on an image returns its
thumbnail (320 px wide, at most 480 high). Only files a capture lists are served; ids and names are
checked against the shapes the plugin writes.
