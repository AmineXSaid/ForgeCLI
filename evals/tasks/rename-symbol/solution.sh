grep -rl calc_total --include='*.py' . | xargs sed -i 's/calc_total/compute_total/g'
