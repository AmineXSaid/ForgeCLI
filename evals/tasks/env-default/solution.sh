sed -i 's/int(os.environ\["PORT"\])/int(os.environ.get("PORT", "8080"))/' app/settings.py
