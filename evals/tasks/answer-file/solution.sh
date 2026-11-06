grep -h ' ok$' logs/*.log | sed 's/.*user=\([^ ]*\).*/\1/' | sort -u | wc -l | tr -d ' ' > answer.txt
