#!/usr/bin/env python3

# Copyright 2022 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""
This script supports visualizing all histograms found in an inspect snapshot.

To use you need to have a snapshot.zip file either generated by `ffx snapshot` or from a crash
report. This also works by dumping the output of `ffx --machine json inspect` to a file.

Example usage:

```
$ unzip /path/to/snapshot.zip
$ python3 snapshot_histograms.py /path/to/unzipped/inspect.json
// open html in browser.
```
"""

import argparse
import json
import sys


def extract_buckets(payload, current_path=[]):
  result = []
  for (key, child) in payload.items():
    if type(child) is not dict:
      continue
    if 'buckets' in child:
      path = current_path + [key]
      result.append(('/'.join(path), child['buckets']))
      continue
    result.extend(extract_buckets(child, current_path + [key]))
  return result


HTML_DATA = """
<html>
  <head>
    <script type="text/javascript" src="https://www.gstatic.com/charts/loader.js"></script>
    <script type="text/javascript">
      google.charts.load('current', {packages: ['corechart', 'table']});
      google.charts.setOnLoadCallback(onLoad);
      const allData = {DATA};
      let showTables = false;

      function drawChart(charts, nodePath, buckets) {
        let data = [['Range', 'Count']];
        for (const bucket of buckets) {
          if (bucket.ceiling === bucket.floor) { continue; }
          data.push([
            { v: bucket.floor, f: `${bucket.floor}-${bucket.ceiling}`},
            bucket.count
          ]);
        }
        let dataTable = google.visualization.arrayToDataTable(data);

        let chartDiv = document.createElement('div');
        chartDiv.classList.add('chart');
        charts.appendChild(chartDiv);

        let tableDiv = document.createElement('div');
        tableDiv.classList.add('table');
        tableDiv.hidden = !showTables;
        charts.appendChild(tableDiv);

        const chart = new google.visualization.ColumnChart(chartDiv);
        chart.draw(dataTable, {
          title: nodePath,
          hAxis: {
            title: 'Ranges',
            logScale: true,
          },
        });

        const table = new google.visualization.Table(tableDiv);
        table.draw(dataTable, {showRowNumber: true, 'max-height': '100px'});
      }

      function render(moniker) {
        if (moniker !== undefined) {
          let charts = document.getElementById('charts');
          charts.innerHTML = '';
          for (const nodePath of Object.keys(allData[moniker])) {
            drawChart(charts, nodePath, allData[moniker][nodePath]);
          }
        }
      }

      function onLoad() {
        const select = document.querySelector('#monikers');
        let selectedMoniker = undefined;
        for (const moniker of Object.keys(allData)) {
          const opt = document.createElement('option');
          opt.innerText = moniker;
          opt.setAttribute('value', moniker);
          select.appendChild(opt);
        }
        select.addEventListener('change', (event) => {
          render(event.target.value);
        });

        const showTablesCheckbox = document.querySelector('#show-tables');
        showTablesCheckbox.addEventListener('change', (event) => {
          showTables = event.target.checked;
                console.log('got change', showTables);
          for (const element of document.getElementsByClassName('table')) {
            console.log(`Changing ${element} to ${!showTables}`);
            element.hidden = !showTables;
          }
        });
      }
    </script>
  </head>
  <body>
    <select id="monikers">
      <option value="" disabled selected>Select moniker</option>
    </select>
    <input type="checkbox" id="show-tables"> Show tables
    <div id="charts"></div>
  </body>
</html>
"""

if __name__ == '__main__':
  parser = argparse.ArgumentParser(
      description='Show histograms in a snapshot.')
  parser.add_argument(
      'filepath', metavar='FILE', type=str, help='path to inspect.json file')
  args = parser.parse_args(sys.argv[1:])
  results = {}
  with open(args.filepath) as f:
    data = json.load(f)
    for response in data:
      if response['payload']:
        buckets = extract_buckets(response['payload'])
        if buckets:
          results[response['moniker']] = dict(buckets)
  html = HTML_DATA.replace('{DATA}', '{}'.format(results))
  print(html)